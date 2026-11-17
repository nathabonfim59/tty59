//! Switcher footer under width pressure: every hint is whole or absent.
//!
//! The footer's hints are one row in a box that clips what does not fit, so on
//! a narrow card the Russian wording pushed the last hint (the selected row's
//! Shift+F10 More) out to the edge, where a single keycap showed. These tests
//! drive the real panel, with the production element tree and no test-only
//! layout nodes, and read where the footer laid each hint out with
//! `debug_bounds` on the hint's own element.
//!
//! A hint counts as shown only if the whole hint sits inside the box that
//! clips it AND it has the size the same hint has when the window gives it all
//! the room it wants. That wide size is measured live, in the same locale, on
//! the same panel, just before narrowing it: a label that wrapped or was
//! squeezed no longer matches its own wide size, whatever font and scale are
//! installed, and a hint pushed past the box fails containment. Merely
//! existing proves nothing here.
//!
//! What a hint carries is read from the label string it was built with
//! (`paint_probe`, observation only): a hint built with no label is a
//! keycap-only hint, one with its label is full.

use gpui::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext, point, px, size};

use super::{CARD_MARGIN, CARD_W, Column};
use crate::core::session::{WindowView, WindowViews, WorkspaceId, WorkspaceStore};
use crate::ui::app::Tty59App;
use crate::ui::app::test_window::harness_with_tabs;
use crate::ui::i18n::{L10nKey, set_locale, t};
use crate::ui::paint_probe;

const WIDE: f32 = 1200.;
const NARROW: f32 = 640.;
const HEIGHT: f32 = 600.;
/// Sub-pixel slack for layout rounding. Not a spacing allowance: anything past
/// half a pixel is a hint outside its box or a hint that changed size.
const EPS: f32 = 0.5;

const HINTS: &str = "switcher-footer-hints";
const CREATE: &str = "switcher-footer-new-workspace";
const MENU_HINT: &str = "switcher-hint-row-menu";

/// A keycap hint's selector. The label note is recorded under the same string.
struct Probe {
    id: &'static str,
    /// Higher is kept longer. More is the protected one.
    rank: u8,
    hint: &'static str,
}

const PROBES: [Probe; 4] = [
    Probe {
        id: "navigate",
        rank: 2,
        hint: "switcher-hint:navigate",
    },
    Probe {
        id: "open",
        rank: 3,
        hint: "switcher-hint:open",
    },
    Probe {
        id: "new-window",
        rank: 1,
        hint: "switcher-hint:new-window",
    },
    Probe {
        id: "more",
        rank: 4,
        hint: MENU_HINT,
    },
];

/// The hints that are a sentence with no keycaps. Whole or absent, like the rest.
const TEXT_HINTS: [(&str, &str); 2] = [
    ("tab-cross", "switcher-hint:tab-cross"),
    ("hold", "switcher-hint:hold"),
];

const RU: &str = "ru-RU";
const ZH: &str = "zh-CN";
const EN: &str = "en";

/// What a hint is showing right now.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Shape {
    Absent,
    KeysOnly,
    Full,
}

/// Every hint's size with all the room it wants.
struct Refs {
    hints: Vec<(&'static str, Bounds<Pixels>)>,
    text: Vec<(&'static str, Bounds<Pixels>)>,
}

struct Fx {
    app: Entity<Tty59App>,
    vcx: VisualTestContext,
    here: WorkspaceId,
    _streams: Vec<crate::daemon::transport::Stream>,
}

impl Fx {
    /// The switcher over three local workspaces, the first picked, in
    /// `locale` at `width` x 600 logical pixels. The caller holds
    /// `paint_probe::scope()` for the length of the test, before this runs,
    /// or no label note is recorded.
    fn open(cx: &mut TestAppContext, locale: &str, width: f32) -> Self {
        let (app, mut vcx, streams) = harness_with_tabs(cx, 1);
        // After the harness, so nothing it initialises can put the locale back.
        set_locale(locale);
        let local = t(L10nKey::TabTooltipMore);
        set_locale(EN);
        let english = t(L10nKey::TabTooltipMore);
        set_locale(locale);
        if locale != EN {
            assert_ne!(
                local, english,
                "setup: {locale} words More differently from English"
            );
        }
        let here = app.update_in(&mut vcx, |app, _, _| app.workspace);
        let now = crate::ui::home::now_secs();
        let views = [
            (here, "alpha", 0),
            (WorkspaceId::new(), "bravo", 60),
            (WorkspaceId::new(), "charlie", 120),
        ]
        .into_iter()
        .map(|(id, label, age): (WorkspaceId, &str, u64)| WindowView {
            id,
            open: true,
            last_active: now.saturating_sub(age),
            label: Some(label.into()),
            ..WindowView::default()
        })
        .collect();
        app.update_in(&mut vcx, |_, _, cx| {
            WorkspaceStore::install_for_test(
                cx,
                WindowViews {
                    views,
                    active: None,
                },
            );
            cx.notify();
        });
        let mut fx = Self {
            app,
            vcx,
            here,
            _streams: streams,
        };
        fx.resize(width);
        fx.app
            .update_in(&mut fx.vcx, |app, window, cx| app.open_switcher(window, cx));
        fx.settle();
        fx.vcx
            .simulate_mouse_move(point(px(2.), px(2.)), None, Modifiers::none());
        fx.settle();
        assert!(
            fx.app.update(&mut fx.vcx, |app, _| app.switcher.is_some()),
            "setup: the switcher is up"
        );
        assert!(
            fx.bounds(HINTS).is_some() && fx.bounds(CREATE).is_some(),
            "setup: the footer is drawn at {width}px in {locale}"
        );
        assert_eq!(
            paint_probe::label(MENU_HINT).as_deref(),
            Some(local),
            "setup: the footer's More is worded in {locale}, not another locale"
        );
        fx
    }

    fn resize(&mut self, width: f32) {
        self.vcx.simulate_resize(size(px(width), px(HEIGHT)));
        self.settle();
    }

    /// Draws and lets deferred work land, twice, as the sibling fixtures do.
    /// Notes are dropped before each draw, so what is read afterwards was
    /// built by the last frame and a label absent from it cannot survive from
    /// the one before.
    fn settle(&mut self) {
        for _ in 0..2 {
            paint_probe::clear();
            self.vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            self.vcx.run_until_parked();
        }
    }

    fn bounds(&mut self, selector: &'static str) -> Option<Bounds<Pixels>> {
        self.vcx.debug_bounds(selector)
    }

    fn set_query(&mut self, text: &'static str) {
        self.app.update_in(&mut self.vcx, |app, window, cx| {
            let query = app.switcher.as_ref().map(|sw| sw.query.clone());
            if let Some(query) = query {
                query.update(cx, |state, cx| state.set_value(text, window, cx));
            }
        });
        self.settle();
    }

    fn set_hold(&mut self, hold: Option<Modifiers>) {
        self.app.update(&mut self.vcx, |app, cx| {
            if let Some(sw) = app.switcher.as_mut() {
                sw.hold = hold;
            }
            // The real hold path notifies; so does this.
            cx.notify();
        });
        self.settle();
    }

    /// The card's horizontal extent, from the same arithmetic the panel lays
    /// itself out with (it is centred in the window).
    fn card(&mut self) -> (f32, f32) {
        let viewport = self.vcx.update(|window, _| window.viewport_size());
        let card_w = CARD_W
            .min(viewport.width.as_f32() - 2. * CARD_MARGIN)
            .max(320.);
        let left = (viewport.width.as_f32() - card_w) / 2.;
        (left, left + card_w)
    }

    /// The label a hint was built with this frame; `None` when it was not
    /// built, `Some("")` for a keycap-only hint.
    fn built_label(&mut self, selector: &'static str) -> Option<String> {
        paint_probe::label(selector)
    }

    /// What each keycap hint is showing.
    fn shapes(&mut self) -> Vec<(&'static str, u8, Shape)> {
        PROBES
            .iter()
            .map(|p| {
                let shape = if self.bounds(p.hint).is_none() {
                    Shape::Absent
                } else if self.built_label(p.hint).is_some_and(|l| l.is_empty()) {
                    Shape::KeysOnly
                } else {
                    Shape::Full
                };
                (p.id, p.rank, shape)
            })
            .collect()
    }

    /// Records every hint's size with room to spare. The current frame must be
    /// a wide one in which all four keycap hints are drawn; the two sentences
    /// are put up and taken down again to measure them. A sentence is measured
    /// only if the wide window actually draws it: it ranks below every keycap
    /// hint, so with a query in the box the card, which is capped, may have no
    /// room left for it, and a sentence that is not drawn has no size to
    /// record. One drawn later with no size on record fails `whole_or_absent`.
    fn references(&mut self, locale: &str) -> Refs {
        let mut hints = Vec::new();
        for p in &PROBES {
            let Some(bounds) = self.bounds(p.hint) else {
                panic!(
                    "setup: at {WIDE}px in {locale} the {} hint must be drawn to measure it",
                    p.id
                );
            };
            hints.push((p.id, bounds));
        }
        let mut text = Vec::new();
        // The sentence reads the same whatever the query is, so it is measured
        // in the first of these in which the wide window draws it: with a
        // picked row (More competes for the room) or with none (it does not).
        for query in ["alpha", "zzz-nothing-matches"] {
            self.set_query(query);
            if let Some(tab) = self.bounds(TEXT_HINTS[0].1) {
                text.push((TEXT_HINTS[0].0, tab));
                break;
            }
        }
        self.set_query("");
        self.set_hold(Some(Modifiers::control()));
        let hold = self
            .bounds(TEXT_HINTS[1].1)
            .expect("setup: a held gesture puts its sentence up");
        text.push((TEXT_HINTS[1].0, hold));
        self.set_hold(None);
        Refs { hints, text }
    }

    /// Opens at WIDE in `locale`, records the references, narrows to `width`.
    fn narrowed(cx: &mut TestAppContext, locale: &str, width: f32) -> (Self, Refs) {
        let mut fx = Self::open(cx, locale, WIDE);
        let refs = fx.references(locale);
        fx.resize(width);
        (fx, refs)
    }

    /// Every way the footer in front of us shows a hint that is not whole.
    /// Empty means every hint drawn is fully inside the box that clips it and,
    /// if it carries its label, is the size it has with room, and the box sits
    /// inside the card without overlapping the New Workspace button.
    fn whole_or_absent(&mut self, refs: &Refs) -> Vec<String> {
        let mut bad = Vec::new();
        let Some(clip) = self.bounds(HINTS) else {
            return bad;
        };
        let (card_left, card_right) = self.card();
        if clip.left().as_f32() < card_left - EPS || clip.right().as_f32() > card_right + EPS {
            bad.push(format!(
                "the hints box {clip:?} leaves the card [{card_left}, {card_right}]"
            ));
        }
        if let Some(create) = self.bounds(CREATE)
            && clip.left().as_f32() < create.right().as_f32() - EPS
        {
            bad.push(format!(
                "the hints box {clip:?} overlaps the New Workspace button {create:?}"
            ));
        }
        let shapes = self.shapes();
        for p in &PROBES {
            let Some(hint) = self.bounds(p.hint) else {
                continue;
            };
            let natural = refs
                .hints
                .iter()
                .find(|(id, _)| *id == p.id)
                .map(|(_, b)| *b)
                .expect("a reference for every probe");
            if !inside(&hint, &clip) {
                bad.push(format!(
                    "{}: hint {hint:?} is not inside the hints box {clip:?}",
                    p.id
                ));
            }
            let shape = shapes
                .iter()
                .find(|(id, ..)| *id == p.id)
                .map(|(_, _, s)| *s);
            match shape {
                Some(Shape::Full) if !same_size(&hint, &natural) => bad.push(format!(
                    "{}: hint is {:?}, not its {:?} with room (squeezed, wrapped or cut)",
                    p.id, hint.size, natural.size
                )),
                Some(Shape::KeysOnly)
                    if (hint.size.height.as_f32() - natural.size.height.as_f32()).abs() > EPS
                        || hint.size.width.as_f32() >= natural.size.width.as_f32() =>
                {
                    bad.push(format!(
                        "{}: keycap-only hint {:?} is not a one-line cut of {:?}",
                        p.id, hint.size, natural.size
                    ))
                }
                _ => {}
            }
        }
        for (id, selector) in TEXT_HINTS {
            let Some(text) = self.bounds(selector) else {
                continue;
            };
            let Some(natural) = refs
                .text
                .iter()
                .find(|(name, _)| *name == id)
                .map(|(_, b)| *b)
            else {
                bad.push(format!(
                    "{id}: sentence {text:?} is drawn but was never drawn with room, so it has no size to be whole against"
                ));
                continue;
            };
            if !inside(&text, &clip) {
                bad.push(format!(
                    "{id}: sentence {text:?} is not inside the hints box {clip:?}"
                ));
            }
            if !same_size(&text, &natural) {
                bad.push(format!(
                    "{id}: sentence is {:?}, not its {:?} with room (squeezed or wrapped)",
                    text.size, natural.size
                ));
            }
        }
        bad
    }

    /// Rank order: a hint never shows more than one ranked above it, and the
    /// protected More shows in full until every other hint is gone.
    fn rank_violations(&mut self) -> Vec<String> {
        let shapes = self.shapes();
        let mut bad = Vec::new();
        for (low, low_rank, low_shape) in &shapes {
            for (high, high_rank, high_shape) in &shapes {
                if low_rank < high_rank && low_shape > high_shape {
                    bad.push(format!(
                        "{low} ({low_shape:?}) shows more than {high} ({high_shape:?}) above it"
                    ));
                }
            }
        }
        if let Some((_, _, more)) = shapes.iter().find(|(id, ..)| *id == "more")
            && *more != Shape::Full
            && shapes
                .iter()
                .any(|(id, _, shape)| *id != "more" && *shape != Shape::Absent)
        {
            bad.push(format!(
                "More is {more:?} while another hint is still drawn: {shapes:?}"
            ));
        }
        bad
    }
}

fn inside(inner: &Bounds<Pixels>, outer: &Bounds<Pixels>) -> bool {
    inner.left().as_f32() >= outer.left().as_f32() - EPS
        && inner.right().as_f32() <= outer.right().as_f32() + EPS
        && inner.top().as_f32() >= outer.top().as_f32() - EPS
        && inner.bottom().as_f32() <= outer.bottom().as_f32() + EPS
}

fn same_size(a: &Bounds<Pixels>, b: &Bounds<Pixels>) -> bool {
    (a.size.width.as_f32() - b.size.width.as_f32()).abs() <= EPS
        && (a.size.height.as_f32() - b.size.height.as_f32()).abs() <= EPS
}

fn assert_clean(what: &str, bad: Vec<String>) {
    assert!(bad.is_empty(), "{what}:\n  {}", bad.join("\n  "));
}

/// The setup every narrow claim stands on: all four wide hints are drawn
/// whole and in full, the selected row's More among them.
fn wide_is_whole(fx: &mut Fx, locale: &str) {
    let shapes = fx.shapes();
    assert!(
        shapes.iter().all(|(_, _, shape)| *shape == Shape::Full),
        "{locale} at {WIDE}px every hint is shown in full: {shapes:?}"
    );
    let refs = fx.references(locale);
    assert_clean(
        &format!("{locale} at {WIDE}px every hint is whole"),
        fx.whole_or_absent(&refs),
    );
}

// ---- Russian at 640: the reported failure ----------------------------------

/// SETUP only: the Russian hints, with all the room they want, are wider than
/// the box the footer has left at 640. This is what makes the next tests about
/// pressure; their own failures are visible hints outside the clip.
#[gpui::test]
fn ru_640_setup_the_wide_hints_are_wider_than_the_room(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (mut fx, refs) = Fx::narrowed(cx, RU, NARROW);
    let natural: f32 = refs
        .hints
        .iter()
        .map(|(_, b)| b.size.width.as_f32())
        .sum::<f32>()
        + 16. * (refs.hints.len() as f32 - 1.);
    let room = fx.bounds(HINTS).expect("the footer is drawn").size.width;
    assert!(
        natural > room.as_f32() + EPS,
        "SETUP: Russian hints want {natural}px but the box has {room:?}"
    );
}

#[gpui::test]
fn ru_640_every_hint_is_whole_or_absent(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (mut fx, refs) = Fx::narrowed(cx, RU, NARROW);
    assert_clean(
        "Russian footer at 640px, selected row",
        fx.whole_or_absent(&refs),
    );
}

#[gpui::test]
fn ru_640_a_typed_query_keeps_every_hint_whole_or_absent(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (mut fx, refs) = Fx::narrowed(cx, RU, NARROW);
    fx.set_query("alpha");
    // The Tab sentence ranks last, so it may legitimately be left out when the
    // keycap hints use the room; what is drawn is checked the same either way.
    let typed = fx.app.update(&mut fx.vcx, |app, cx| {
        app.switcher.as_ref().map(|sw| sw.text(cx))
    });
    assert_eq!(
        typed.as_deref(),
        Some("alpha"),
        "setup: the query is in the box"
    );
    assert_clean(
        "Russian footer at 640px with a query in the box",
        fx.whole_or_absent(&refs),
    );
}

#[gpui::test]
fn ru_640_the_selected_rows_more_is_shown_in_full(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (mut fx, refs) = Fx::narrowed(cx, RU, NARROW);
    let natural = refs
        .hints
        .iter()
        .find(|(id, _)| *id == "more")
        .map(|(_, b)| *b)
        .expect("more");
    let clip = fx.bounds(HINTS).expect("the footer is drawn");
    let more = fx.bounds(MENU_HINT);
    let Some(more) = more else {
        panic!("the picked row's More hint is not drawn at all");
    };
    assert_eq!(
        fx.built_label(MENU_HINT).as_deref(),
        Some(t(L10nKey::TabTooltipMore)),
        "More carries its own label"
    );
    assert!(inside(&more, &clip), "More {more:?} outside box {clip:?}");
    assert!(
        same_size(&more, &natural),
        "More is {:?}, not its {:?} with room",
        more.size,
        natural.size
    );
}

#[gpui::test]
fn ru_640_lower_hints_give_way_before_more_does(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (mut fx, _) = Fx::narrowed(cx, RU, NARROW);
    assert_clean(
        "Russian footer at 640px, hint priority",
        fx.rank_violations(),
    );
}

// ---- Chinese and English at 640: controls ----------------------------------

#[gpui::test]
fn zh_640_every_hint_is_whole_or_absent_and_more_is_kept(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (mut fx, refs) = Fx::narrowed(cx, ZH, NARROW);
    assert_clean("Chinese footer at 640px", fx.whole_or_absent(&refs));
    assert_clean(
        "Chinese footer at 640px, hint priority",
        fx.rank_violations(),
    );
    assert!(
        fx.shapes()
            .iter()
            .any(|(id, _, s)| *id == "more" && *s == Shape::Full),
        "More is shown in full: {:?}",
        fx.shapes()
    );
}

#[gpui::test]
fn en_640_every_hint_is_whole_or_absent_and_more_is_kept(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let (mut fx, refs) = Fx::narrowed(cx, EN, NARROW);
    assert_clean("English footer at 640px", fx.whole_or_absent(&refs));
    assert_clean(
        "English footer at 640px, hint priority",
        fx.rank_violations(),
    );
    assert!(
        fx.shapes()
            .iter()
            .any(|(id, _, s)| *id == "more" && *s == Shape::Full),
        "More is shown in full: {:?}",
        fx.shapes()
    );
}

// ---- Wide: nothing is taken away --------------------------------------------

#[gpui::test]
fn ru_wide_shows_every_hint_in_full(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut fx = Fx::open(cx, RU, WIDE);
    wide_is_whole(&mut fx, RU);
}

#[gpui::test]
fn zh_wide_shows_every_hint_in_full(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut fx = Fx::open(cx, ZH, WIDE);
    wide_is_whole(&mut fx, ZH);
}

#[gpui::test]
fn en_wide_shows_every_hint_in_full(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut fx = Fx::open(cx, EN, WIDE);
    wide_is_whole(&mut fx, EN);
}

// ---- Where More has no business: still absent, and the rest still whole -----

/// One locale's walk through the states in which Shift+F10 does nothing, at
/// 640. Each is undone before the next, as the sibling footer test does, and
/// each asserts the footer is drawn (so an absent More is not an absent
/// footer), More is absent, and whatever hints remain are whole.
fn no_more_states_stay_whole(cx: &mut TestAppContext, locale: &str) {
    fn quiet(fx: &mut Fx, refs: &Refs, state: &str) {
        assert!(
            fx.bounds(HINTS).is_some(),
            "{state}: the footer's hints box is drawn"
        );
        assert!(
            fx.bounds(MENU_HINT).is_none() && fx.built_label(MENU_HINT).is_none(),
            "{state}: no More hint"
        );
        assert_clean(state, fx.whole_or_absent(refs));
    }

    let _probe = paint_probe::scope();
    let (mut fx, refs) = Fx::narrowed(cx, locale, NARROW);

    fx.app
        .update(&mut fx.vcx, |app, cx| app.switcher_focus(Column::Right, cx));
    fx.settle();
    quiet(&mut fx, &refs, &format!("{locale} tab column"));
    fx.app
        .update(&mut fx.vcx, |app, cx| app.switcher_focus(Column::Left, cx));
    fx.settle();
    assert!(
        fx.built_label(MENU_HINT).is_some(),
        "{locale}: back on the workspace column More is built again"
    );

    fx.set_hold(Some(Modifiers::control()));
    assert!(
        fx.built_label(TEXT_HINTS[1].1).is_some(),
        "{locale}: setup: the held footer shows its own sentence"
    );
    quiet(&mut fx, &refs, &format!("{locale} held gesture"));
    fx.set_hold(None);

    fx.set_query("zzz-nothing-matches");
    quiet(&mut fx, &refs, &format!("{locale} no row under the cursor"));
    fx.set_query("");

    let here = fx.here;
    fx.app.update_in(&mut fx.vcx, |app, window, cx| {
        app.switcher_rename(here, window, cx)
    });
    fx.settle();
    quiet(&mut fx, &refs, &format!("{locale} rename open"));
}

#[gpui::test]
fn ru_640_no_more_states_stay_whole(cx: &mut TestAppContext) {
    no_more_states_stay_whole(cx, RU);
}

#[gpui::test]
fn zh_640_no_more_states_stay_whole(cx: &mut TestAppContext) {
    no_more_states_stay_whole(cx, ZH);
}

#[gpui::test]
fn en_640_no_more_states_stay_whole(cx: &mut TestAppContext) {
    no_more_states_stay_whole(cx, EN);
}

#[gpui::test]
fn ru_640_the_create_form_shows_no_more_hint(cx: &mut TestAppContext) {
    let _probe = paint_probe::scope();
    let mut fx = Fx::open(cx, RU, NARROW);
    fx.app.update_in(&mut fx.vcx, |app, window, cx| {
        app.open_workspace_form(window, cx)
    });
    fx.settle();
    assert!(
        fx.bounds(MENU_HINT).is_none() && fx.built_label(MENU_HINT).is_none(),
        "the create form has no row menu, so no More hint"
    );
}
