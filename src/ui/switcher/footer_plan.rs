//! Which of the footer's hints fit, decided from measured widths.
//!
//! The footer is one row: the New Workspace button, then the hints. Hints used
//! to sit in a box that clipped whatever overflowed, so a long wording pushed
//! the last hint, the selected row's Shift+F10 More, out to the edge where a
//! single keycap showed. Here every hint is either whole or not drawn, and
//! what gives way, in order, is decided on widths the caller measured with the
//! real font, never on character counts.
//!
//! This file is pure arithmetic: no gpui, no fonts, no windows. The caller
//! measures text and hands the widths in, so the whole policy and every layout
//! constant it depends on can be tested with plain numbers.
//!
//! The policy, by rank (higher is kept longer):
//!
//! 1. Everything shown in full. If it fits, that is the answer.
//! 2. Lowest rank first, a hint loses its words and keeps its keycaps. A
//!    sentence has no keycaps to keep, so it is simply dropped at this step.
//! 3. Lowest rank first, whole hints are dropped.
//! 4. Only the protected hint (More) is left. It gives up its words only now,
//!    and is dropped only if even its keycaps do not fit.

/// What a hint is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Shape {
    /// Not drawn at all.
    Absent,
    /// Its keycaps with no words. Never used for a hint that has no keycaps.
    KeysOnly,
    /// Keycaps and words.
    Full,
}

/// Ranks, higher kept longer. Visual order is not rank order.
pub(super) const RANK_SENTENCE: u8 = 0;
pub(super) const RANK_MACHINE: u8 = 1;
pub(super) const RANK_NEW_WINDOW: u8 = 2;
pub(super) const RANK_NAVIGATE: u8 = 3;
pub(super) const RANK_OPEN: u8 = 4;
pub(super) const RANK_MORE: u8 = 5;

// ---- the footer's own geometry --------------------------------------------
//
// The element tree is built from these same constants, so the width the
// planner reasons about is the width layout is given.

/// `floating_surface` draws `border_1()` on the card: one pixel each side, so
/// the footer's content box is the card's width less two of them.
pub(super) const CARD_BORDER: f32 = 1.;
/// The footer row's padding and the gap between its button and its hints box.
pub(super) const FOOTER_PAD_L: f32 = 8.;
pub(super) const FOOTER_PAD_R: f32 = 10.;
pub(super) const FOOTER_GAP: f32 = 16.;
/// The gap between hints inside the hints box.
pub(super) const HINTS_GAP: f32 = 16.;
/// The gap between a hint's keycaps and its words.
pub(super) const HINT_GAP: f32 = 6.;

/// The New Workspace button: horizontal padding, icon, and icon-to-text gap.
pub(super) const CREATE_PAD: f32 = 10.;
pub(super) const CREATE_ICON: f32 = 10.;
pub(super) const CREATE_GAP: f32 = 8.;

/// A keycap's horizontal padding (`dialog::keycap`: `px(4.)`) and its minimum
/// width, the dialog's own constant.
pub(super) const KEYCAP_PAD: f32 = 4.;
pub(super) const KEYCAP_MIN: f32 = crate::ui::dialog::KEYCAP;

/// Text sizes, in sixteenths of the interface rem: a hint's words
/// (`rems(11.5 / 16.)`), a keycap's glyph (`rems(11. / 16.)`) and the create
/// button's words (`rems(12.5 / 16.)`).
pub(super) const HINT_TEXT: f32 = 11.5;
pub(super) const KEYCAP_TEXT: f32 = 11.;
pub(super) const CREATE_TEXT: f32 = 12.5;

/// A keycap's width given its glyph's measured, already rounded-up width: the
/// glyph and its padding, but never under the minimum.
pub(super) fn keycap_w(glyph_w: f32) -> f32 {
    (glyph_w + 2. * KEYCAP_PAD).max(KEYCAP_MIN)
}

/// A hint's width: its keycaps and, if it has words, the words, with the hint
/// gap between each neighbouring pair. A keycap-only hint has no words and so
/// no gap after its last keycap.
pub(super) fn hint_w(glyph_ws: &[f32], words_w: Option<f32>) -> f32 {
    let caps: f32 = glyph_ws.iter().map(|w| keycap_w(*w)).sum();
    let parts = glyph_ws.len() + usize::from(words_w.is_some());
    caps + words_w.unwrap_or(0.) + HINT_GAP * parts.saturating_sub(1) as f32
}

/// The New Workspace button's width given its measured label.
pub(super) fn create_w(label_w: f32) -> f32 {
    2. * CREATE_PAD + CREATE_ICON + CREATE_GAP + label_w
}

/// The room the hints box has: the card, less its border, the footer's
/// padding, the button and the gap after it. Negative when the button alone
/// does not fit.
pub(super) fn hints_budget(card_w: f32, create_label_w: f32) -> f32 {
    card_w - 2. * CARD_BORDER - FOOTER_PAD_L - FOOTER_PAD_R - create_w(create_label_w) - FOOTER_GAP
}

/// One hint as the planner sees it, listed in visual order.
#[derive(Clone, Copy, Debug)]
pub(super) struct Item {
    pub(super) rank: u8,
    /// The one hint that is kept as long as it can be.
    pub(super) protected: bool,
    /// Width with its words.
    pub(super) full: f32,
    /// Width of its keycaps alone; `None` for a hint with no keycaps.
    pub(super) keys_only: Option<f32>,
}

fn width_of(item: &Item, shape: Shape) -> Option<f32> {
    match shape {
        Shape::Absent => None,
        Shape::Full => Some(item.full),
        Shape::KeysOnly => item.keys_only,
    }
}

/// Whether the hints in these shapes, with `gap` between neighbours, fit in
/// `budget`. An exact fit fits.
fn fits(items: &[Item], shapes: &[Shape], budget: f32, gap: f32) -> bool {
    let (mut total, mut shown) = (0., 0usize);
    for (item, shape) in items.iter().zip(shapes) {
        if let Some(w) = width_of(item, *shape) {
            total += w;
            shown += 1;
        }
    }
    total + gap * shown.saturating_sub(1) as f32 <= budget
}

/// How each hint is drawn, in the order given.
pub(super) fn plan(items: &[Item], budget: f32, gap: f32) -> Vec<Shape> {
    let mut shapes = vec![Shape::Full; items.len()];
    if fits(items, &shapes, budget, gap) {
        return shapes;
    }
    // Lowest rank first; equal ranks keep their visual order.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| items[i].rank);

    // Lower hints lose their words, or go if they have none to lose.
    for &i in &order {
        if items[i].protected {
            continue;
        }
        shapes[i] = match items[i].keys_only {
            Some(_) => Shape::KeysOnly,
            None => Shape::Absent,
        };
        if fits(items, &shapes, budget, gap) {
            return shapes;
        }
    }
    // Then lower hints go whole.
    for &i in &order {
        if items[i].protected {
            continue;
        }
        shapes[i] = Shape::Absent;
        if fits(items, &shapes, budget, gap) {
            return shapes;
        }
    }
    // Only the protected hint is left, and it does not fit in full.
    for &i in &order {
        if !items[i].protected {
            continue;
        }
        shapes[i] = match items[i].keys_only {
            Some(_) => Shape::KeysOnly,
            None => Shape::Absent,
        };
        if fits(items, &shapes, budget, gap) {
            return shapes;
        }
        shapes[i] = Shape::Absent;
    }
    shapes
}

#[cfg(test)]
mod tests {
    //! POST-IMPLEMENTATION tests: written after the planner, to pin its
    //! boundaries. The live footer is covered by `footer_clip_tests`.

    use super::*;

    const GAP: f32 = HINTS_GAP;

    fn item(rank: u8, full: f32, keys_only: Option<f32>) -> Item {
        Item {
            rank,
            protected: false,
            full,
            keys_only,
        }
    }

    fn more(full: f32, keys_only: f32) -> Item {
        Item {
            rank: RANK_MORE,
            protected: true,
            full,
            keys_only: Some(keys_only),
        }
    }

    /// The footer's hints in visual order with a sentence ahead of them: the
    /// sentence, Navigate, Open, New window, Machine, More. Widths are chosen
    /// so every step is a different sum.
    fn row() -> Vec<Item> {
        vec![
            item(RANK_SENTENCE, 100., None),
            item(RANK_NAVIGATE, 90., Some(40.)),
            item(RANK_OPEN, 80., Some(20.)),
            item(RANK_NEW_WINDOW, 70., Some(30.)),
            item(RANK_MACHINE, 60., Some(50.)),
            more(110., 60.),
        ]
    }

    fn full_width(items: &[Item]) -> f32 {
        items.iter().map(|i| i.full).sum::<f32>() + GAP * (items.len() as f32 - 1.)
    }

    use Shape::{Absent as A, Full as F, KeysOnly as K};

    #[test]
    fn everything_shows_in_full_when_it_fits() {
        let items = row();
        let need = full_width(&items);
        assert_eq!(plan(&items, need + 50., GAP), vec![F; 6]);
    }

    #[test]
    fn an_exact_fit_fits_and_a_hair_less_does_not() {
        let items = row();
        let need = full_width(&items);
        assert_eq!(plan(&items, need, GAP), vec![F; 6]);
        // One step less: the sentence goes first (no keycaps to keep), which
        // is the whole saving, 100 + one gap.
        assert_eq!(plan(&items, need - 0.25, GAP), vec![A, F, F, F, F, F]);
    }

    #[test]
    fn the_sentence_goes_before_any_keycap_hint_loses_its_words() {
        let items = row();
        let need = full_width(&items);
        let after_sentence = need - 100. - GAP;
        assert_eq!(
            plan(&items, after_sentence, GAP),
            vec![A, F, F, F, F, F],
            "exactly what is left fits"
        );
        // A hair under that and Machine, the lowest keycap hint, is cut to its
        // keycaps; nothing above it is touched.
        assert_eq!(
            plan(&items, after_sentence - 0.25, GAP),
            vec![A, F, F, F, K, F]
        );
    }

    #[test]
    fn lower_hints_lose_their_words_lowest_first() {
        let items = row();
        let base = full_width(&items) - 100. - GAP;
        // Machine keys-only saves 10, New window saves 40, Navigate 50, Open 60.
        assert_eq!(plan(&items, base - 10., GAP), vec![A, F, F, F, K, F]);
        assert_eq!(plan(&items, base - 10. - 0.25, GAP), vec![A, F, F, K, K, F]);
        assert_eq!(plan(&items, base - 50., GAP), vec![A, F, F, K, K, F]);
        assert_eq!(plan(&items, base - 50. - 0.25, GAP), vec![A, K, F, K, K, F]);
        assert_eq!(plan(&items, base - 100., GAP), vec![A, K, F, K, K, F]);
        assert_eq!(
            plan(&items, base - 100. - 0.25, GAP),
            vec![A, K, K, K, K, F]
        );
    }

    #[test]
    fn whole_hints_go_only_after_every_lower_label_is_down_to_keycaps() {
        let items = row();
        // Everything but More at keycaps: 40 + 20 + 30 + 50 = 140, More 110,
        // four gaps of 16.
        let all_keys = 40. + 20. + 30. + 50. + 110. + 4. * GAP;
        assert_eq!(plan(&items, all_keys, GAP), vec![A, K, K, K, K, F]);
        // Under it, Machine goes first.
        assert_eq!(plan(&items, all_keys - 0.25, GAP), vec![A, K, K, K, A, F]);
        // Then New window, Navigate, Open: lowest rank first, never More.
        let without_machine = 40. + 20. + 30. + 110. + 3. * GAP;
        assert_eq!(plan(&items, without_machine, GAP), vec![A, K, K, K, A, F]);
        assert_eq!(
            plan(&items, without_machine - 0.25, GAP),
            vec![A, K, K, A, A, F]
        );
        let without_new_window = 40. + 20. + 110. + 2. * GAP;
        assert_eq!(
            plan(&items, without_new_window, GAP),
            vec![A, K, K, A, A, F]
        );
        assert_eq!(
            plan(&items, without_new_window - 0.25, GAP),
            vec![A, A, K, A, A, F]
        );
        let without_navigate = 20. + 110. + GAP;
        assert_eq!(plan(&items, without_navigate, GAP), vec![A, A, K, A, A, F]);
        assert_eq!(
            plan(&items, without_navigate - 0.25, GAP),
            vec![A, A, A, A, A, F]
        );
    }

    #[test]
    fn more_is_whole_until_it_stands_alone_then_loses_its_words_last() {
        let items = row();
        assert_eq!(plan(&items, 110., GAP), vec![A, A, A, A, A, F]);
        assert_eq!(plan(&items, 110. - 0.25, GAP), vec![A, A, A, A, A, K]);
        assert_eq!(plan(&items, 60., GAP), vec![A, A, A, A, A, K]);
        assert_eq!(plan(&items, 60. - 0.25, GAP), vec![A; 6]);
    }

    #[test]
    fn nothing_fits_in_nothing() {
        let items = row();
        assert_eq!(plan(&items, 0., GAP), vec![A; 6]);
        assert_eq!(plan(&items, -40., GAP), vec![A; 6]);
        assert_eq!(plan(&[], -40., GAP), Vec::<Shape>::new());
    }

    #[test]
    fn a_hold_footer_is_its_sentence_whole_or_absent() {
        let items = [item(RANK_SENTENCE, 100., None)];
        assert_eq!(plan(&items, 100., GAP), vec![F]);
        assert_eq!(plan(&items, 100. - 0.25, GAP), vec![A]);
    }

    #[test]
    fn the_protected_hint_is_the_real_one_not_the_highest_rank_present() {
        // No More at all (a tab column, a rename): Open is the highest rank
        // left but is not protected, so it goes when nothing else can give.
        let items = vec![
            item(RANK_NAVIGATE, 90., Some(40.)),
            item(RANK_OPEN, 80., Some(20.)),
        ];
        assert_eq!(plan(&items, 90. + 80. + GAP, GAP), vec![F, F]);
        assert_eq!(plan(&items, 40. + 80. + GAP, GAP), vec![K, F]);
        assert_eq!(plan(&items, 40. + 20. + GAP, GAP), vec![K, K]);
        assert_eq!(plan(&items, 20., GAP), vec![A, K]);
        assert_eq!(plan(&items, 20. - 0.25, GAP), vec![A, A]);
    }

    #[test]
    fn equal_ranks_give_way_in_visual_order() {
        let items = vec![item(1, 50., Some(10.)), item(1, 50., Some(10.))];
        assert_eq!(plan(&items, 50. + 50. + GAP, GAP), vec![F, F]);
        assert_eq!(plan(&items, 10. + 50. + GAP, GAP), vec![K, F]);
        assert_eq!(plan(&items, 10. + 10. + GAP, GAP), vec![K, K]);
    }

    #[test]
    fn a_shape_never_exceeds_one_ranked_above_it() {
        let items = row();
        let need = full_width(&items);
        let mut budget = need + 20.;
        while budget > -20. {
            let shapes = plan(&items, budget, GAP);
            for (i, low) in items.iter().enumerate() {
                for (j, high) in items.iter().enumerate() {
                    assert!(
                        !(low.rank < high.rank && shapes[i] > shapes[j]),
                        "at {budget}: {shapes:?} shows rank {} above rank {}",
                        low.rank,
                        high.rank
                    );
                }
            }
            let shown: f32 = items
                .iter()
                .zip(&shapes)
                .filter_map(|(item, shape)| width_of(item, *shape))
                .sum();
            let count = shapes.iter().filter(|s| **s != A).count();
            assert!(
                count == 0 || shown + GAP * (count as f32 - 1.) <= budget,
                "at {budget}: {shapes:?} overflows"
            );
            if shapes[5] != F {
                assert!(
                    shapes[..5].iter().all(|s| *s == A),
                    "at {budget}: More is {:?} with {shapes:?} still drawn",
                    shapes[5]
                );
            }
            budget -= 1.;
        }
    }

    // ---- layout constants, against the real keycap -------------------------

    #[test]
    fn a_keycap_is_its_glyph_and_padding_but_never_under_the_minimum() {
        assert_eq!(KEYCAP_MIN, crate::ui::dialog::KEYCAP);
        assert_eq!(KEYCAP_MIN, 18.);
        // Narrow glyphs ride the minimum; wide ones grow with their padding.
        assert_eq!(keycap_w(0.), 18.);
        assert_eq!(keycap_w(10.), 18.);
        assert_eq!(keycap_w(10.5), 18.5);
        assert_eq!(keycap_w(30.), 38.);
    }

    #[test]
    fn a_keys_only_hint_is_its_keycaps_and_their_gaps_and_no_words() {
        // Two arrows at the minimum width: 18 + 6 + 18, no gap after the last.
        assert_eq!(hint_w(&[7., 7.], None), 42.);
        // One keycap alone has no gaps at all.
        assert_eq!(hint_w(&[7.], None), 18.);
        // A wide glyph (a word on a non-Mac keymap): 8 padding around 24.
        assert_eq!(hint_w(&[24., 7.], None), 32. + 6. + 18.);
        // The same hint with words adds one more gap and the words.
        assert_eq!(hint_w(&[7., 7.], Some(55.)), 42. + 6. + 55.);
        // A sentence has no keycaps: just its words.
        assert_eq!(hint_w(&[], Some(55.)), 55.);
    }

    #[test]
    fn the_buttons_and_the_boxs_room_come_from_the_footers_own_geometry() {
        // 10 + 10 padding, a 10 icon, an 8 gap, then the label.
        assert_eq!(create_w(100.), 20. + 10. + 8. + 100.);
        // 760 card, 2 border, 8 + 10 padding, the button, a 16 gap.
        assert_eq!(hints_budget(760., 100.), 760. - 2. - 18. - 138. - 16.);
        // A label too wide for the card leaves no room at all.
        assert!(hints_budget(320., 400.) < 0.);
    }
}
