//! One keyboard stop for a list that moves it from row to row.
//!
//! A long list builds only the rows its viewport reaches, so it cannot give
//! every row a focus handle and cannot find a mounted row's handle from an
//! event. It keeps ONE handle instead. The row the list says holds the keyboard
//! tracks it (and only that row does, in a given frame); moving the keyboard to
//! another row is a change of which row tracks the handle, so the handle stays
//! focused and nothing has to be focused again.
//!
//! What is left is asking for focus when the target may not be built yet: from
//! the keyboard action that opens the panel, or once a status that was still
//! loading has landed. Nothing here polls, re-arms or repairs focus while
//! drawing. A request is a [`Pending`] left in a cell, and the tracked row's
//! own prepaint settles it, in this order:
//!
//! 1. it publishes that the row was mounted, for the generation the frame was
//!    rendered with (`seen`);
//! 2. it takes the request, and only if the request is for that generation and
//!    that window; a request is consumed once, whatever then becomes of it;
//! 3. it hands the request to the caller, which acts after the frame.
//!
//! `Window::on_next_frame` is not usable for this: the platform runs those
//! callbacks BEFORE the next frame is drawn, and the test platform never runs
//! them at all.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, FocusHandle, Pixels, Window, canvas, prelude::*,
};

/// What a [`Pending`] asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingKind {
    /// Put the keyboard on the tracked row.
    Entry,
    /// Scroll the tracked row fully into view.
    Reveal,
}

/// A request waiting for the row it is about to act on to be mounted.
#[derive(Clone)]
pub(crate) struct Pending {
    /// The generation the request was made in. A request only ever settles in
    /// a frame that was rendered with the same one.
    pub(crate) ticket: u64,
    /// The window it was made in. Another window's prepaint never settles it.
    pub(crate) window: AnyWindowHandle,
    /// What had the keyboard when it was made. An entry is dropped if that is
    /// no longer what has it when it comes to act.
    pub(crate) origin: Option<FocusHandle>,
    pub(crate) kind: PendingKind,
}

/// The single handle and the two cells shared with the row that tracks it.
pub(crate) struct Roving {
    /// The handle the tracked row tracks. A tab stop, so Tab reaches the row
    /// that currently tracks it.
    pub(crate) handle: FocusHandle,
    /// The generation and bounds of the last mounted tracked row, written in
    /// its prepaint.
    seen: Rc<Cell<Option<(u64, Bounds<Pixels>)>>>,
    pending: Rc<RefCell<Option<Pending>>>,
}

impl Roving {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            handle: cx.focus_handle().tab_stop(true),
            seen: Rc::default(),
            pending: Rc::default(),
        }
    }

    /// A new generation: whatever was published for an earlier one no longer
    /// counts, and a request made in an earlier one is dropped. Returns the new
    /// generation.
    pub(crate) fn bump(&self, ticket: &mut u64) -> u64 {
        *ticket += 1;
        self.seen.set(None);
        self.pending.borrow_mut().take();
        *ticket
    }

    /// Leave `pending` to be settled by the next frame that mounts the tracked
    /// row for its generation. Replaces any request already waiting.
    pub(crate) fn request(&self, pending: Pending) {
        *self.pending.borrow_mut() = Some(pending);
    }

    /// Drop a waiting request, if there is one.
    pub(crate) fn cancel(&self) {
        self.pending.borrow_mut().take();
    }

    /// The bounds of the tracked row, if it was mounted by a frame rendered
    /// with generation `ticket`.
    pub(crate) fn mounted(&self, ticket: u64) -> Option<Bounds<Pixels>> {
        match self.seen.get() {
            Some((seen, bounds)) if seen == ticket => Some(bounds),
            _ => None,
        }
    }

    /// The element the tracked row carries: a canvas as large as the row whose
    /// prepaint does the three steps in the module comment. `ticket` is the
    /// generation read while rendering, captured by value. `settle` gets the
    /// request and the row's bounds, with the window, and runs during
    /// prepaint, so it must only schedule work (`App::defer`) or touch
    /// non-entity state.
    pub(crate) fn probe(
        &self,
        ticket: u64,
        settle: impl FnOnce(Pending, Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let seen = self.seen.clone();
        let pending = self.pending.clone();
        canvas(
            move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                seen.set(Some((ticket, bounds)));
                let here = window.window_handle();
                let taken = pending
                    .borrow_mut()
                    .take_if(|request| request.ticket == ticket && request.window == here);
                if let Some(request) = taken {
                    settle(request, bounds, window, cx);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }
}
