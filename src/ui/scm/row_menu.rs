//! The context menu of a source-control row: a Changes file or a History
//! commit.
//!
//! The toolkit's `.context_menu(..)` keeps the open menu in the row's element
//! state and wires its own dismiss subscription back to that same state, a
//! reference cycle: the `PopupMenu` entity outlives the row for good, and a
//! test that opens one cannot end without gpui's leak check firing. This is the
//! same menu (the toolkit's own `PopupMenu`, built by the same builders, with
//! its own Escape, arrows, Enter, item clicks and click-outside dismissal)
//! owned by the row instead, with nothing pointing back:
//!
//! * the state is `Window::use_keyed_state`, which gpui keeps exactly while
//!   the row is built in consecutive frames and drops the frame after it is
//!   not. A row that leaves the list, moves to another group, or whose panel
//!   is no longer shown takes its menu with it;
//! * the one strong handle on the `PopupMenu` is in that state, and the
//!   dismiss and focus-out subscriptions are registered on the state's own
//!   context, so their closures hold a weak self. Dropping the state releases
//!   the menu and both subscriptions together;
//! * what the rest of the panel needs to know ("a row menu is open", and
//!   which Changes row, which a scrolled list must keep built) is
//!   `ScmPanelState::menu_row` / `menu_commit`, written when the menu opens and
//!   cleared when it closes **or** when a state that has a menu open is
//!   released, never from paint. Every write names the menu it is for
//!   (`menu_instance`), so a late end of an older menu, even of the same row,
//!   cannot clear what a newer one recorded.

use gpui::{
    Anchor, AnyElement, AnyWindowHandle, App, Context, DismissEvent, ElementId, Entity, EntityId,
    FocusHandle, Focusable as _, InteractiveElement as _, IntoElement as _, ParentElement as _,
    Pixels, Point, Styled as _, Subscription, WeakEntity, Window, anchored, deferred, div, px,
};
use gpui_component::menu::PopupMenu;

use crate::ui::app::Tty59App;
use crate::ui::scm::state::{MenuOrigin, ScmGroup, ScmPanelState};

/// How far the menu keeps from the window's edge: the margin the toolkit's own
/// context menu and popovers snap with.
const SNAP_MARGIN: Pixels = px(8.);

/// The row a menu belongs to, as the panel records it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum RowOwner {
    /// A Changes file row, by group and path.
    Change(ScmGroup, String),
    /// A History row, by commit.
    Commit(String),
}

/// Record `menu` as the open one of `owner`: exactly one kind of owner is on
/// record afterwards, the other is cleared with the instance it had.
fn record_into(scm: &mut ScmPanelState, owner: &RowOwner, menu: EntityId) {
    match owner {
        RowOwner::Change(group, path) => {
            scm.menu_row = Some((*group, path.clone()));
            scm.menu_commit = None;
        }
        RowOwner::Commit(oid) => {
            scm.menu_commit = Some(oid.clone());
            scm.menu_row = None;
        }
    }
    scm.menu_instance = Some(menu);
    // An origin belongs to one menu: a record that names a new one starts without it.
    scm.menu_origin = None;
}

/// `record_into`, with the focus the menu gives back. One write, so the origin
/// is on record exactly as long as the menu it belongs to.
fn record_open_into(scm: &mut ScmPanelState, owner: &RowOwner, menu: EntityId, origin: MenuOrigin) {
    record_into(scm, owner, menu);
    scm.menu_origin = Some(origin);
}

/// Clear the record if, and only if, `menu` is the menu on record: the end of
/// an older menu, of this row or of another, is not ours to write. Whether it
/// changed.
fn clear_from(scm: &mut ScmPanelState, menu: EntityId) -> bool {
    let ours = scm.menu_instance == Some(menu);
    if ours {
        scm.menu_row = None;
        scm.menu_commit = None;
        scm.menu_instance = None;
        scm.menu_origin = None;
    }
    ours
}

// Writes to the panel's record queued, for the test that proves a state that
// never opened a menu queues none.
#[cfg(test)]
thread_local! {
    static WRITES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn writes() -> usize {
    WRITES.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(super) fn reset_writes() {
    WRITES.with(|n| n.set(0));
}

/// Run `write` on the panel's state at the end of the current effect cycle:
/// this can be reached while the app is itself being updated, and an entity
/// cannot be re-entered. `write` says whether it changed anything.
fn queue(
    app: &WeakEntity<Tty59App>,
    cx: &mut App,
    write: impl FnOnce(&mut ScmPanelState) -> bool + 'static,
) {
    #[cfg(test)]
    WRITES.with(|n| n.set(n.get() + 1));
    let app = app.clone();
    cx.defer(move |cx| {
        // The app being gone means the window is closing; there is no record left to keep.
        let _ = app.update(cx, |this, cx| {
            if write(&mut this.scm) {
                cx.notify();
            }
        });
    });
}

fn record_open(
    app: &WeakEntity<Tty59App>,
    owner: &RowOwner,
    menu: EntityId,
    origin: MenuOrigin,
    cx: &mut App,
) {
    let owner = owner.clone();
    queue(app, cx, move |scm| {
        record_open_into(scm, &owner, menu, origin);
        true
    });
}

fn record_close(app: &WeakEntity<Tty59App>, menu: EntityId, cx: &mut App) {
    queue(app, cx, move |scm| clear_from(scm, menu));
}

/// Whether `handle` is what has the keyboard focus: it or something inside it
/// is focused, or it is still the focused handle although it has left the
/// tree (its panel went). A focus that moved to something else is not ours.
fn owns_focus(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    handle.contains_focused(window, cx)
        || window.focused(cx).is_some_and(|focused| focused == *handle)
}

/// The focus a new menu of this row gives back when it ends. While an earlier
/// menu of the row is still open (`open` is its remembered focus), the original
/// is carried over as it was, an original of NOTHING included: falling back to
/// what is focused now would hand back the earlier menu's own handle. With no
/// menu open, it is what is focused now.
fn original_focus<T: Clone>(
    open: Option<&Option<T>>,
    now: impl FnOnce() -> Option<T>,
) -> Option<T> {
    match open {
        Some(previous) => previous.clone(),
        None => now(),
    }
}

/// The focus a menu asked for now gives back, when `focused` is what has the
/// keyboard now. If that is the handle of the menu on record (`origin`: its
/// handle, and the focus it found), the menu is being replaced, and the one
/// to give back is the original one it found, an original of NOTHING included.
/// Anything else is what is focused now.
fn focus_to_return<T: Clone + PartialEq>(
    origin: Option<(&T, &Option<T>)>,
    focused: Option<T>,
) -> Option<T> {
    match (origin, focused) {
        (Some((handle, original)), Some(focused)) if focused == *handle => original.clone(),
        (_, focused) => focused,
    }
}

/// An open menu and everything that belongs to it.
struct Open {
    menu: Entity<PopupMenu>,
    /// Where the pointer was when it was asked for, in window coordinates.
    position: Point<Pixels>,
    /// What had focus before the menu took it, given back when the menu ends
    /// while it still has the focus.
    previous_focus: Option<FocusHandle>,
    window: AnyWindowHandle,
    _dismiss: Subscription,
    _focus_out: Subscription,
}

/// One row's menu: closed until asked for.
pub(super) struct RowMenu {
    app: WeakEntity<Tty59App>,
    owner: RowOwner,
    open: Option<Open>,
}

impl RowMenu {
    /// The row's menu state, alive for as long as the row keeps being built.
    /// `owner` is only called when the state is first created.
    pub(super) fn of(
        key: impl Into<ElementId>,
        owner: impl FnOnce() -> RowOwner,
        app: WeakEntity<Tty59App>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        window.use_keyed_state(key, cx, move |_, cx| {
            // The state is released when its row stops being built. If a menu is
            // open at that moment, whatever the panel recorded for it goes with
            // it; a state that never opened one has nothing to undo.
            cx.on_release(|this: &mut RowMenu, cx: &mut App| this.released(cx))
                .detach();
            RowMenu {
                app,
                owner: owner(),
                open: None,
            }
        })
    }

    /// Open the row's menu at `position`, built by `build`. An empty menu is
    /// not opened. Asking again replaces the menu that is open.
    pub(super) fn open(
        this: &Entity<Self>,
        position: Point<Pixels>,
        build: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu,
        window: &mut Window,
        cx: &mut App,
    ) {
        this.update(cx, |state, cx| state.show(position, build, window, cx));
    }

    fn show(
        &mut self,
        position: Point<Pixels>,
        build: impl FnOnce(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The menu on record, if any, is read now, when the row is asked for its
        // menu, and never while painting. The panel being gone means the window
        // is closing: there is nothing to open a menu in.
        let Ok(on_record) = self.app.read_with(cx, |app, _| app.scm.menu_origin.clone()) else {
            return;
        };
        let menu = PopupMenu::build(window, cx, build);
        if menu.read(cx).is_empty() {
            return;
        }
        let handle = menu.focus_handle(cx);
        // What had the focus before the first menu took it. A menu asked for
        // again by this row while the first is still up gives back the original
        // focus, not the first menu's own handle; so does a menu of another
        // row, whose first menu has not been dismissed yet when it opens.
        let previous_focus =
            original_focus(self.open.as_ref().map(|open| &open.previous_focus), || {
                focus_to_return(
                    on_record
                        .as_ref()
                        .map(|origin| (&origin.handle, &origin.original)),
                    window.focused(cx),
                )
            });
        // Its own Escape, a click outside it and a picked item all end in this.
        // Only the menu this state has open ends it: the end of one it has
        // replaced is not an end of this one.
        let dismiss = cx.subscribe_in(&menu, window, |this, menu, _: &DismissEvent, window, cx| {
            if this.open.as_ref().is_some_and(|open| open.menu == *menu) {
                this.close(window, cx);
            }
        });
        // Focus that moves away without a dismissal (another panel took it) ends
        // the menu too; a focus that went somewhere else is left where it went.
        let focus_out = cx.on_focus_out(&handle, window, |this, _, window, cx| {
            this.close(window, cx);
        });
        window.focus(&handle, cx);
        let id = menu.entity_id();
        // Replacing an open menu drops it, its subscriptions and its entity.
        self.open = Some(Open {
            menu,
            position,
            previous_focus,
            window: window.window_handle(),
            _dismiss: dismiss,
            _focus_out: focus_out,
        });
        let origin = MenuOrigin {
            handle,
            original: self
                .open
                .as_ref()
                .and_then(|open| open.previous_focus.clone()),
        };
        record_open(&self.app, &self.owner, id, origin, cx);
        cx.notify();
    }

    /// Close the menu, giving the focus back to what had it before if the menu
    /// still has it, and leaving it where it is if it went somewhere else.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else {
            return;
        };
        if owns_focus(&open.menu.focus_handle(cx), window, cx)
            && let Some(previous) = &open.previous_focus
        {
            window.focus(previous, cx);
        }
        record_close(&self.app, open.menu.entity_id(), cx);
        cx.notify();
    }

    /// The state is going (its row, its group or its panel is no longer built).
    /// A menu still open takes its record with it, and gives the focus back if
    /// it still owns it, both at the end of the cycle: the window may be
    /// mid-update right now. A state with no menu open does nothing at all.
    fn released(&mut self, cx: &mut App) {
        let Some(open) = self.open.take() else {
            return;
        };
        record_close(&self.app, open.menu.entity_id(), cx);
        let handle = open.menu.focus_handle(cx);
        if let Some(previous) = open.previous_focus.clone() {
            let window = open.window;
            cx.defer(move |cx| {
                // The window being gone means there is no focus left to give back.
                let _ = window.update(cx, |_, window, cx| {
                    if owns_focus(&handle, window, cx) {
                        window.focus(&previous, cx);
                    }
                });
            });
        }
    }

    /// The menu as the row draws it, when it is open: the toolkit's own element
    /// tree for a context menu (a window-sized layer that keeps the scroll wheel
    /// off the list under it, the menu anchored at the pointer and snapped back
    /// inside the window). Reads only; nothing is written or focused while
    /// painting.
    pub(super) fn overlay(this: &Entity<Self>, window: &Window, cx: &App) -> Option<AnyElement> {
        let state = this.read(cx);
        let open = state.open.as_ref()?;
        let size = window.bounds().size;
        Some(
            deferred(
                anchored().child(
                    div()
                        .w(size.width)
                        .h(size.height)
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .child(
                            anchored()
                                .position(open.position)
                                .snap_to_window_with_margin(SNAP_MARGIN)
                                .anchor(Anchor::TopLeft)
                                .child(open.menu.clone()),
                        ),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change() -> RowOwner {
        RowOwner::Change(ScmGroup::Untracked, "b.rs".to_string())
    }

    fn commit() -> RowOwner {
        RowOwner::Commit("0123abcd".to_string())
    }

    /// What the panel has on record. The instance is an opaque `EntityId`,
    /// compared with the ids the test constructed and never with a number.
    fn on_record(scm: &ScmPanelState) -> (Option<String>, Option<String>, Option<EntityId>) {
        (
            scm.menu_row.as_ref().map(|(_, path)| path.clone()),
            scm.menu_commit.clone(),
            scm.menu_instance,
        )
    }

    /// The end of an older menu of the SAME row, arriving after a newer one of
    /// that row recorded itself, is not the newer one's end.
    #[test]
    fn a_stale_end_of_an_older_menu_of_the_same_row_does_not_clear_the_newer_one() {
        let mut scm = ScmPanelState::default();
        let (older, newer) = (EntityId::from(1u64), EntityId::from(2u64));
        assert_ne!(older, newer);
        record_into(&mut scm, &change(), older);
        record_into(&mut scm, &change(), newer);
        assert!(
            !clear_from(&mut scm, older),
            "the older end changes nothing"
        );
        assert_eq!(on_record(&scm), (Some("b.rs".into()), None, Some(newer)));
        assert!(clear_from(&mut scm, newer), "the newer one's own end does");
        assert_eq!(on_record(&scm), (None, None, None));
    }

    /// A menu on a Changes row replaced by one on a History row (or the other
    /// way round) leaves exactly the new owner on record, and the replaced
    /// menu's late end clears nothing.
    #[test]
    fn replacing_a_menu_by_one_of_the_other_kind_leaves_only_the_new_owner() {
        let mut scm = ScmPanelState::default();
        let (first, second, third) = (
            EntityId::from(1u64),
            EntityId::from(2u64),
            EntityId::from(3u64),
        );
        record_into(&mut scm, &change(), first);
        record_into(&mut scm, &commit(), second);
        assert_eq!(
            on_record(&scm),
            (None, Some("0123abcd".into()), Some(second)),
            "the Changes record is gone with its menu"
        );
        assert!(!clear_from(&mut scm, first));
        record_into(&mut scm, &change(), third);
        assert_eq!(
            on_record(&scm),
            (Some("b.rs".into()), None, Some(third)),
            "and the other way round"
        );
    }

    /// Nothing on record, or someone else's menu on record: nothing to clear.
    #[test]
    fn only_the_menu_on_record_clears_it() {
        let mut scm = ScmPanelState::default();
        let (recorded, other, absent) = (
            EntityId::from(4u64),
            EntityId::from(5u64),
            EntityId::from(9u64),
        );
        assert!(!clear_from(&mut scm, absent), "nothing recorded");
        record_into(&mut scm, &commit(), recorded);
        assert!(!clear_from(&mut scm, other), "another menu's end");
        assert_eq!(
            on_record(&scm),
            (None, Some("0123abcd".into()), Some(recorded))
        );
    }

    /// A menu opened on another row while the first is still up (its dismissal
    /// has not arrived) gives back the focus the first one found; the handle
    /// of the first menu is only what is focused at that moment.
    #[test]
    fn a_menu_replacing_another_gives_back_the_focus_the_first_one_found() {
        let (first_menu, before, elsewhere) = (10u32, 3u32, 4u32);
        let on_record = (&first_menu, &Some(before));
        assert_eq!(
            focus_to_return(Some(on_record), Some(first_menu)),
            Some(before),
            "the first menu has the focus: the focus from before it is the original"
        );
        assert_eq!(
            focus_to_return(Some((&first_menu, &None)), Some(first_menu)),
            None,
            "an original of nothing stays nothing, not the first menu's handle"
        );
        assert_eq!(
            focus_to_return(Some(on_record), Some(elsewhere)),
            Some(elsewhere),
            "the focus is somewhere else: that is the one"
        );
        assert_eq!(
            focus_to_return(Some(on_record), None),
            None,
            "nothing is focused: nothing"
        );
        assert_eq!(
            focus_to_return(None, Some(first_menu)),
            Some(first_menu),
            "no menu on record: what is focused now"
        );
    }

    /// The origin is written with the record of its menu and goes with it,
    /// and an older menu's end neither clears nor replaces it.
    #[gpui::test]
    fn the_origin_is_on_record_exactly_as_long_as_its_menu(cx: &mut gpui::TestAppContext) {
        let (menu_handle, original, other_menu, other_original) = cx.update(|cx| {
            (
                cx.focus_handle(),
                cx.focus_handle(),
                cx.focus_handle(),
                cx.focus_handle(),
            )
        });
        let origin = |handle: &FocusHandle, original: Option<&FocusHandle>| MenuOrigin {
            handle: handle.clone(),
            original: original.cloned(),
        };
        let mut scm = ScmPanelState::default();
        let (older, newer) = (EntityId::from(1u64), EntityId::from(2u64));
        record_open_into(&mut scm, &change(), older, origin(&menu_handle, None));
        assert!(
            scm.menu_origin
                .as_ref()
                .is_some_and(|o| o.handle == menu_handle && o.original.is_none()),
            "recorded with its menu, an original of nothing kept"
        );
        record_open_into(
            &mut scm,
            &commit(),
            newer,
            origin(&other_menu, Some(&other_original)),
        );
        assert!(
            !clear_from(&mut scm, older),
            "the older end changes nothing"
        );
        assert!(
            scm.menu_origin.as_ref().is_some_and(|o| {
                o.handle == other_menu && o.original.as_ref() == Some(&other_original)
            }),
            "the newer menu's origin is still the one on record"
        );
        assert!(clear_from(&mut scm, newer));
        assert!(scm.menu_origin.is_none(), "it goes with its menu");
        record_open_into(
            &mut scm,
            &change(),
            older,
            origin(&menu_handle, Some(&original)),
        );
        record_into(&mut scm, &change(), newer);
        assert!(
            scm.menu_origin.is_none(),
            "a record of a menu with no origin does not keep the previous menu's"
        );
    }

    /// A menu asked for again while an earlier one of the row is open gives
    /// back the focus the FIRST one found, and an original of nothing stays
    /// nothing instead of becoming the earlier menu's own handle.
    #[test]
    fn a_reopened_menu_keeps_the_original_focus_even_when_it_was_none() {
        let now_is_the_old_menu = || Some(7u32);
        assert_eq!(
            original_focus(Some(&None), now_is_the_old_menu),
            None,
            "an original of nothing is kept"
        );
        assert_eq!(
            original_focus(Some(&Some(3u32)), now_is_the_old_menu),
            Some(3),
            "an original focus is kept"
        );
        assert_eq!(
            original_focus(None, now_is_the_old_menu),
            Some(7),
            "with no menu open it is what is focused now"
        );
    }
}
