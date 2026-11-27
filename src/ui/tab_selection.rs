//! The sidebar's multi-tab selection: the tabs picked with a secondary click
//! (Ctrl, or Cmd on macOS) or a shift click, to be moved or closed together.
//!
//! Kept by [`TabId`], never by index: indices shift whenever a tab closes or
//! moves, and a selection that followed an index would quietly pick up the
//! bystander that slid into the gap. Fewer than two tabs is no selection at
//! all: a single picked row is just the active tab.

use std::collections::HashSet;

use tty59_core::core::machine::TabId;

#[derive(Debug, Default, Clone)]
pub(crate) struct TabSelection {
    ids: HashSet<TabId>,
    /// Where a shift click's range starts: the last row secondary-clicked.
    anchor: Option<TabId>,
}

impl TabSelection {
    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    pub(crate) fn contains(&self, id: TabId) -> bool {
        self.ids.contains(&id)
    }

    pub(crate) fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// A secondary click on `clicked`. The first one seeds the selection with
    /// `active`, so Ctrl-clicking one other row picks two; every one flips
    /// `clicked` and makes it the anchor. Whatever leaves fewer than two
    /// picked ends the selection.
    pub(crate) fn toggle(&mut self, active: TabId, clicked: TabId) {
        if self.ids.is_empty() {
            self.ids.insert(active);
        }
        if !self.ids.remove(&clicked) {
            self.ids.insert(clicked);
        }
        self.anchor = Some(clicked);
        self.settle();
    }

    /// A shift click on `clicked`: the rows from the anchor (else `active`)
    /// to `clicked`, both ends included, in `order`, the rows as the sidebar
    /// draws them. A folded group's rows are not drawn, so they are not in
    /// `order` and are never taken. The anchor stays where it was, so a
    /// second shift click re-draws the range from the same row.
    pub(crate) fn select_range(&mut self, active: TabId, clicked: TabId, order: &[TabId]) {
        let anchor = self.anchor.filter(|a| order.contains(a)).unwrap_or(active);
        let (Some(from), Some(to)) = (
            order.iter().position(|&id| id == anchor),
            order.iter().position(|&id| id == clicked),
        ) else {
            return;
        };
        let (lo, hi) = (from.min(to), from.max(to));
        self.ids = order[lo..=hi].iter().copied().collect();
        self.anchor = Some(anchor);
        self.settle();
    }

    /// Drop the tabs `alive` no longer answers for: closed, or gone to
    /// another window. Answers whether that ended the selection.
    pub(crate) fn retain(&mut self, alive: impl Fn(TabId) -> bool) -> bool {
        if self.ids.is_empty() {
            return false;
        }
        self.ids.retain(|&id| alive(id));
        if self.anchor.is_some_and(|a| !alive(a)) {
            self.anchor = None;
        }
        self.settle();
        self.ids.is_empty()
    }

    /// The picked tabs in `order`, the order the sidebar draws them in.
    pub(crate) fn in_order(&self, order: &[TabId]) -> Vec<TabId> {
        order
            .iter()
            .copied()
            .filter(|id| self.ids.contains(id))
            .collect()
    }

    fn settle(&mut self) {
        if self.ids.len() < 2 {
            self.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<TabId> {
        (0..n).map(|_| TabId::new()).collect()
    }

    #[test]
    fn first_toggle_seeds_with_the_active_tab() {
        let t = ids(3);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[2]);
        assert_eq!(s.in_order(&t), vec![t[0], t[2]]);
    }

    #[test]
    fn toggling_the_last_extra_row_off_ends_the_selection() {
        let t = ids(3);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[1]);
        s.toggle(t[0], t[2]);
        s.toggle(t[0], t[2]);
        assert_eq!(s.in_order(&t), vec![t[0], t[1]]);
        s.toggle(t[0], t[1]);
        assert!(s.is_empty());
    }

    #[test]
    fn toggling_the_active_tab_alone_selects_nothing() {
        let t = ids(2);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[0]);
        assert!(s.is_empty());
    }

    #[test]
    fn the_active_tab_can_leave_a_selection_of_others() {
        let t = ids(3);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[1]);
        s.toggle(t[0], t[2]);
        s.toggle(t[0], t[0]);
        assert_eq!(s.in_order(&t), vec![t[1], t[2]]);
    }

    #[test]
    fn range_runs_from_the_active_tab_without_an_anchor() {
        let t = ids(5);
        let mut s = TabSelection::default();
        s.select_range(t[1], t[3], &t);
        assert_eq!(s.in_order(&t), vec![t[1], t[2], t[3]]);
    }

    #[test]
    fn range_runs_upward_too() {
        let t = ids(5);
        let mut s = TabSelection::default();
        s.select_range(t[3], t[0], &t);
        assert_eq!(s.in_order(&t), t[0..=3].to_vec());
    }

    #[test]
    fn range_starts_at_the_last_secondary_clicked_row() {
        let t = ids(6);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[4]);
        s.select_range(t[0], t[2], &t);
        assert_eq!(s.in_order(&t), vec![t[2], t[3], t[4]]);
        // The anchor stays: a second shift click re-draws from it.
        s.select_range(t[0], t[5], &t);
        assert_eq!(s.in_order(&t), vec![t[4], t[5]]);
    }

    #[test]
    fn range_skips_rows_that_are_not_drawn() {
        // Visual order across groups, with a folded group's t[2], t[3]
        // left out of what the sidebar draws.
        let t = ids(6);
        let drawn = vec![t[5], t[0], t[1], t[4]];
        let mut s = TabSelection::default();
        s.select_range(t[5], t[4], &drawn);
        assert_eq!(s.in_order(&t), vec![t[0], t[1], t[4], t[5]]);
        assert!(!s.contains(t[2]) && !s.contains(t[3]));
        assert_eq!(s.in_order(&drawn), drawn);
    }

    #[test]
    fn range_onto_the_anchor_itself_is_no_selection() {
        let t = ids(3);
        let mut s = TabSelection::default();
        s.select_range(t[1], t[1], &t);
        assert!(s.is_empty());
    }

    #[test]
    fn range_to_a_row_not_drawn_changes_nothing() {
        let t = ids(4);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[1]);
        s.select_range(t[0], t[3], &t[..3]);
        assert_eq!(s.in_order(&t), vec![t[0], t[1]]);
    }

    #[test]
    fn retain_drops_closed_tabs_and_ends_below_two() {
        let t = ids(4);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[1]);
        s.toggle(t[0], t[2]);
        assert!(!s.retain(|id| id != t[1]));
        assert_eq!(s.in_order(&t), vec![t[0], t[2]]);
        assert!(s.retain(|id| id != t[2]));
        assert!(s.is_empty());
    }

    #[test]
    fn retain_forgets_a_gone_anchor() {
        let t = ids(5);
        let mut s = TabSelection::default();
        s.toggle(t[0], t[1]);
        s.toggle(t[0], t[4]);
        s.retain(|id| id != t[4]);
        // The anchor went with t[4]; the range falls back to the active tab.
        s.select_range(t[2], t[3], &t);
        assert_eq!(s.in_order(&t), vec![t[2], t[3]]);
    }

    #[test]
    fn retain_on_no_selection_reports_nothing_ended() {
        let mut s = TabSelection::default();
        assert!(!s.retain(|_| false));
    }
}
