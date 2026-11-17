//! The settings page's own controls.
//!
//! Settings is drawn to one design, and that design is quieter than the rest
//! of the app: ink at a handful of fixed strengths instead of the theme's named
//! roles, hairline rings instead of borders, and controls that sit on the page
//! rather than on cards. Everything here derives from the active theme's
//! foreground and background, so the page follows every theme — light, dark,
//! and the tinted ones in between — without a palette of its own.
//!
//! Sizes are in rems off a 16px base, written as the pixel sizes the design
//! calls for, so the page still grows with the interface font.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::Focusable as _;
use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement,
    MouseButton, Pixels, Rems, SharedString, Stateful, Window, div, point, prelude::*, px, rems,
};
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, Icon, h_flex, v_flex};

use crate::ui::presets::{lift, settings_wash};
use crate::ui::theme::{FocusRing, RingPlacement};

/// A size from the design, in pixels at the default 16px interface font.
pub(crate) fn fs(px_at_16: f32) -> Rems {
    rems(px_at_16 / 16.)
}

/// Records an element's own paints for the tests that read them back
/// (`paint_probe`); compiled only into those tests.
#[cfg(test)]
pub(crate) fn probe<E: gpui::Styled>(key: &'static str, mut element: E) -> E {
    crate::ui::paint_probe::note(key, &mut element);
    element
}

/// The key-context entry [`mark`] writes: its value is the control's own element
/// id. Compiled only into tests.
#[cfg(test)]
pub(crate) const STOP_MARK: &str = "tty59-stop";

/// Names a control to the keyboard tests: puts `id` on the element's key
/// context under [`STOP_MARK`], so a test reads which control holds focus from
/// `window.context_stack()` of the live focused path. It makes no handle and no
/// tab stop and matches no binding; a production build never has it.
#[cfg(test)]
pub(crate) fn mark<E: gpui::InteractiveElement>(element: E, id: impl std::fmt::Display) -> E {
    let mut context = gpui::KeyContext::default();
    context.set(STOP_MARK, id.to_string());
    element.key_context(context)
}

/// The focus handle of the control drawn under `key`, kept by the framework
/// for exactly as long as the control is drawn: keyed state is scoped to the
/// element-id path (the ids of the identified elements above it, plus the
/// component name), so two same-id controls get a handle each only under
/// parents that carry distinct ids (an anonymous div adds nothing to the
/// path); callers keep their ids unique. A control that is no longer drawn
/// takes its handle (and the focus on it) with it. No registry, nothing
/// repaired while rendering.
///
/// The same keyed state also carries what a stop needs to bring itself into
/// view when the keyboard reaches it (see [`reveal`]): where it was last
/// painted, the scroll owners the current render named, and the focus-in
/// subscription that acts on them. All of it lives and dies with the control.
struct StopState {
    handle: gpui::FocusHandle,
    /// The control's bounds as the LAST frame's prepaint measured them; reset
    /// to `None` by every render and set again only by that frame's prepaint,
    /// so `Some` means painted by the frame that drew the current request.
    painted: Rc<Cell<Option<gpui::Bounds<Pixels>>>>,
    /// What this render asked for: the scrollers to bring the control into
    /// view in, inner to outer. Overwritten every render, never kept from an
    /// earlier request, so a closed and re-opened page cannot leave a stale
    /// owner here.
    request: Rc<RefCell<RevealRequest>>,
    _focus_in: gpui::Subscription,
}

#[derive(Default)]
struct RevealRequest {
    scrolls: Vec<gpui::ScrollHandle>,
    /// Whether the ring stands outside the control (its reach then belongs to
    /// what must be visible); an inside ring stays within the control's box.
    outside: bool,
}

type Painted = Rc<Cell<Option<gpui::Bounds<Pixels>>>>;

fn stop_state(
    key: &ElementId,
    window: &mut Window,
    cx: &mut App,
) -> (gpui::FocusHandle, Painted, Rc<RefCell<RevealRequest>>) {
    let state = window.use_keyed_state(key.clone(), cx, |window, cx| {
        let handle = cx.focus_handle();
        let painted: Painted = Rc::new(Cell::new(None));
        let request = Rc::new(RefCell::new(RevealRequest::default()));
        // Weak: the deferred redraw below checks the state is still mounted.
        let alive = cx.weak_entity();
        let focus_in = window.on_focus_in(&handle, cx, {
            let (handle, painted, request) = (handle.clone(), painted.clone(), request.clone());
            move |window, cx| reveal(window, cx, &handle, &painted, &request, &alive)
        });
        StopState {
            handle,
            painted,
            request,
            _focus_in: focus_in,
        }
    });
    let state = state.read(cx);
    (
        state.handle.clone().tab_stop(true),
        state.painted.clone(),
        state.request.clone(),
    )
}

/// The focus handle of a control that is not a [`Stop`] itself (it builds its own
/// element, ring and handlers) but opts into the same keyboard reveal: the keyed
/// handle (tab stop) under `key`, and, only when `scrolls` is not empty, the
/// pinned measuring child to add to the control. `Stop` takes its handle the same
/// way, so the state, subscription, request overwrite and measurement exist once.
///
/// This render's request replaces the last one whole and the control's measured
/// bounds start again from "not painted yet". `outside`: the control's ring
/// stands outside its box, so its reach is part of what must be visible. The
/// control must be a positioned box (`relative`) for the child to lie over it.
pub(crate) fn armed_focus(
    key: &ElementId,
    scrolls: Vec<gpui::ScrollHandle>,
    outside: bool,
    window: &mut Window,
    cx: &mut App,
) -> (gpui::FocusHandle, Option<AnyElement>) {
    let (handle, painted, request) = stop_state(key, window, cx);
    *request.borrow_mut() = RevealRequest {
        scrolls: scrolls.clone(),
        outside,
    };
    painted.set(None);
    let measure = (!scrolls.is_empty()).then(|| {
        // The same box as the ring layer (`ring_layer`): pinned to the control's
        // origin and size, not left to the auto position a padded or flex
        // control would give an absolute child.
        gpui::canvas(
            move |bounds, _, _| painted.set(Some(bounds)),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    });
    (handle, measure)
}

/// How far the content must move (positive = down) to bring `rect` into `view`
/// by the least movement (a rect taller than the view aligns its top), already
/// limited to what the scroller can actually do: its offset stays within
/// `[-max, 0]`. The result is the movement that WILL happen, so the next,
/// outer scroller is planned from where the rect really ends up.
pub(crate) fn reveal_delta(
    view: gpui::Bounds<Pixels>,
    rect: gpui::Bounds<Pixels>,
    offset_y: Pixels,
    max_y: Pixels,
) -> Pixels {
    let want = if rect.size.height >= view.size.height || rect.top() < view.top() {
        view.top() - rect.top()
    } else if rect.bottom() > view.bottom() {
        view.bottom() - rect.bottom()
    } else {
        return px(0.);
    };
    let target = (offset_y + want).max(-max_y).min(px(0.));
    target - offset_y
}

/// One scroller in a reveal plan: its viewport, vertical offset and furthest
/// scroll, as the last frame left them.
#[derive(Clone, Copy)]
pub(crate) struct Level {
    pub(crate) view: gpui::Bounds<Pixels>,
    pub(crate) offset_y: Pixels,
    pub(crate) max_y: Pixels,
}

/// The effective movement of each level, inner to outer, for `rect` (already
/// grown by whatever must also show). Each level moves by [`reveal_delta`] and
/// the rect is carried by exactly that effective movement into the next.
pub(crate) fn reveal_plan(mut rect: gpui::Bounds<Pixels>, levels: &[Level]) -> Vec<Pixels> {
    levels
        .iter()
        .map(|level| {
            if level.view.size.height <= px(0.) {
                return px(0.);
            }
            let delta = reveal_delta(level.view, rect, level.offset_y, level.max_y);
            rect.origin.y += delta;
            delta
        })
        .collect()
}

/// The focus-in action of a stop that opted into scrolling. It runs once per
/// focus arrival, after the draw that showed the focus, so `painted` is that
/// draw's measurement; only for the keyboard (a click on a half-hidden control
/// does not move the page under the pointer); vertical only, x kept. gpui
/// ignores `refresh()` inside this phase, so the redraw is queued with
/// `Window::defer` and runs only if the state is still mounted and the control
/// still holds focus.
fn reveal(
    window: &mut Window,
    cx: &mut App,
    handle: &gpui::FocusHandle,
    painted: &Painted,
    request: &Rc<RefCell<RevealRequest>>,
    alive: &gpui::WeakEntity<StopState>,
) {
    if !window.last_input_was_keyboard() {
        return;
    }
    let Some(mut rect) = painted.get() else {
        return;
    };
    let (scrolls, outside) = {
        let request = request.borrow();
        (request.scrolls.clone(), request.outside)
    };
    if scrolls.is_empty() {
        return;
    }
    if outside {
        let by = cx.theme().focus_ring_offset() + crate::ui::theme::COMMIT_RING_W;
        rect = gpui::Bounds {
            origin: point(rect.origin.x - by, rect.origin.y - by),
            size: gpui::size(rect.size.width + by * 2.0, rect.size.height + by * 2.0),
        };
    }
    let levels: Vec<Level> = scrolls
        .iter()
        .map(|scroll| Level {
            view: scroll.bounds(),
            offset_y: scroll.offset().y,
            max_y: scroll.max_offset().y,
        })
        .collect();
    let mut moved = false;
    for (scroll, delta) in scrolls.iter().zip(reveal_plan(rect, &levels)) {
        if delta != px(0.) {
            let offset = scroll.offset();
            scroll.set_offset(point(offset.x, offset.y + delta));
            moved = true;
        }
    }
    if moved {
        let (alive, handle) = (alive.clone(), handle.clone());
        window.defer(cx, move |window, _| {
            if alive.upgrade().is_some() && handle.is_focused(window) {
                window.refresh();
            }
        });
    }
}

/// The one place the settings page names the shared keyboard ring. It is a
/// decorative child of the control itself, not a box around it: an absolute
/// layer over the control's own box, holding the shared ring, drawn outside
/// the control's edge by the installed gap and only for keyboard focus (a click
/// that focuses the control does not light it). The control stays the one item
/// its parent lays out, so stretch, block fill, shrink, `flex_1`, anchors and
/// absolute placement behave exactly as they did bare; an absolute child is
/// out of flow, so the control's own gap, padding and children are untouched.
/// Needs a positioned control (see [`Stop`]); has no id, listener or hitbox.
///
/// `keyboard_only`: a stop's ring lights for keyboard focus only (a click that
/// focuses it does not); a text field's lights on any focus, as the field's own
/// caret does. `tag` names the layer's ring `{tag}-ring` in tests and is never
/// evaluated otherwise.
fn ring_layer(
    handle: &gpui::FocusHandle,
    placement: RingPlacement,
    radius: Pixels,
    keyboard_only: bool,
    tag: impl FnOnce() -> String,
) -> AnyElement {
    let ring = FocusRing::new(handle, placement, radius, div().flex_1())
        .keyboard_only(keyboard_only)
        .fill();
    #[cfg(test)]
    let ring = ring.tagged(format!("{}-ring", tag()));
    #[cfg(not(test))]
    let _ = tag;
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .flex_col()
        .child(ring)
        .into_any_element()
}

/// A key pressed with no modifier held. The arrow and Home/End keys a stop
/// answers to are answered only plain: a chord with Ctrl, Alt, Shift, Super or
/// Fn is not the control's, is not consumed, and goes on to whatever binds it
/// (the app's navigation chords, a window manager's).
pub(crate) fn plain_key(ev: &gpui::KeyDownEvent) -> bool {
    let m = &ev.keystroke.modifiers;
    !(m.control || m.alt || m.shift || m.platform || m.function)
}

type KeyHandler = Rc<dyn Fn(&gpui::KeyDownEvent, &mut Window, &mut App)>;

/// A keyboard tab stop on an existing control: the control (a stateful div
/// that already carries its id and click handler) takes a keyed focus handle
/// and a keyboard-only ring layer as a child, and nothing else; it remains the
/// item its parent lays out. Enter and Space fire the control's own click, as
/// gpui does for any focused element with one.
///
/// The control must be a positioned box (`relative`, or already `absolute`) so
/// the ring layer lies over it, and must not clip its overflow, or the ring
/// outside its edge would be clipped with it.
#[derive(IntoElement)]
pub(crate) struct Stop {
    key: ElementId,
    radius: Pixels,
    child: Stateful<Div>,
    live: bool,
    placement: RingPlacement,
    on_key: Option<KeyHandler>,
    /// The scrollers to bring the control into view in when the keyboard
    /// reaches it, inner to outer. Empty: it never scrolls anything.
    scrolls: Vec<gpui::ScrollHandle>,
}

pub(crate) fn stop(key: impl Into<ElementId>, radius: Pixels, child: Stateful<Div>) -> Stop {
    Stop {
        key: key.into(),
        radius,
        child,
        live: true,
        placement: RingPlacement::Outside,
        on_key: None,
        scrolls: Vec::new(),
    }
}

impl Stop {
    /// Opts the control into scrolling: when the keyboard reaches it, `outer`
    /// (the page's scroller) is moved the least that shows it. `None` (no
    /// settings page, or a control outside any scroller) opts out. Explicit per
    /// control: nothing infers which scroller holds a control.
    pub(crate) fn in_scroll(mut self, outer: Option<gpui::ScrollHandle>) -> Self {
        self.scrolls.extend(outer);
        self
    }

    /// An inner scroller (a list inside the page) that holds the control: it is
    /// moved first, then the outer ones, each by what it can actually move.
    pub(crate) fn in_inner_scroll(mut self, inner: Option<gpui::ScrollHandle>) -> Self {
        if let Some(inner) = inner {
            self.scrolls.insert(0, inner);
        }
        self
    }

    fn scrolls(mut self, scrolls: Vec<gpui::ScrollHandle>) -> Self {
        self.scrolls = scrolls;
        self
    }

    /// Draws the shared ring INSIDE the control's edge by the installed gap
    /// instead of outside it: for a control in a clipping container (a scrolling
    /// list), where an outside ring would be cut off or run into its neighbour.
    /// Same ring, radius and installed gap/stroke/ink; only the placement.
    pub(crate) fn inside(mut self) -> Self {
        self.placement = RingPlacement::Inside;
        self
    }

    /// `false` leaves the control exactly as it was: no handle, no ring layer.
    /// A disabled control, or one with nothing to do, is not a stop.
    pub(crate) fn live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }
    /// Keys the control answers to beyond Enter and Space (arrows).
    pub(crate) fn on_key(
        mut self,
        f: impl Fn(&gpui::KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_key = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Stop {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !self.live {
            return self.child.into_any_element();
        }
        let (handle, measure) = armed_focus(
            &self.key,
            self.scrolls.clone(),
            self.placement == RingPlacement::Outside,
            window,
            cx,
        );
        let child = self.child.track_focus(&handle);
        #[cfg(test)]
        let child = mark(child, &self.key);
        // The callback is for the stop ITSELF: a key listener on this node also
        // hears the keys that bubble up from a focusable descendant, and those
        // are that descendant's, so only act while this very handle is focused.
        let owner = handle.clone();
        let child = match self.on_key {
            Some(on_key) => child.on_key_down(move |ev, window, cx| {
                if owner.is_focused(window) {
                    on_key(ev, window, cx)
                }
            }),
            None => child,
        };
        child
            .children(measure)
            .child(ring_layer(
                &handle,
                self.placement,
                self.radius,
                true,
                || self.key.to_string(),
            ))
            .into_any_element()
    }
}

/// The page's ink at the strengths the design uses, plus the few surfaces it
/// lifts off the page.
#[derive(Clone, Copy)]
pub(crate) struct Tk {
    pub dark: bool,
    pub fg: Hsla,
    pub page: Hsla,
    pub k04: Hsla,
    pub k05: Hsla,
    pub k06: Hsla,
    pub k08: Hsla,
    pub k15: Hsla,
    pub k3: Hsla,
    /// Decorative marks (the search glass): cached, held to 3:1 on the
    /// settings fills they sit on. Text that says something uses `sub`.
    pub glyph: Hsla,
    pub k45: Hsla,
    /// Secondary text that carries information: descriptions, counts, Reset,
    /// addresses, empty states. Cached per theme, floored on the page fills.
    pub sub: Hsla,
    /// An off switch's track. Light: floored at 3:1 on the page fills. Dark: the
    /// faint 16% wash it always was, deliberately under 3:1; the knob carries
    /// the state.
    pub switch_off: Hsla,
    /// A group heading: quieter than a label, but still read at a glance.
    pub heading: Hsla,
    /// A nav item at rest.
    pub nav: Hsla,
    /// A raised button's fill.
    pub btn: Hsla,
    /// A popover's fill.
    pub menu: Hsla,
    /// A switch or slider knob at rest.
    pub knob: Hsla,
    /// The chosen cell of a segmented control.
    pub chip: Hsla,
    pub warn: Hsla,
    pub warn_text: Hsla,
    /// The Unsaved meta of a host row (the open host row it is painted on).
    pub unsaved: Hsla,
    pub danger: Hsla,
    pub ok: Hsla,
}

impl Tk {
    pub(crate) fn of(cx: &App) -> Self {
        let theme = cx.theme();
        let dark = theme.is_dark();
        let fg = theme.foreground;
        let page = theme.background;
        let a = |light: f32, dark_a: f32| fg.opacity(if dark { dark_a } else { light });
        let ink = crate::ui::theme::settings_ink(cx);
        let role = |c: u32| -> Hsla { gpui::rgb(c).into() };
        // The washes step toward the cached shared pole (the foreground on
        // every theme but the few where it cannot hold its own fills).
        let pole = role(crate::ui::theme::fill_pole(cx));
        let wash = |(light, dark_a): (f32, f32)| pole.opacity(if dark { dark_a } else { light });
        // Light controls sit a step above the page in the theme's own hue, so
        // a cream theme gets cream fields and a white one stays white. Opaque:
        // the page carries the window's opacity, a control should not.
        let raised = Hsla {
            a: 1.,
            ..lift(page, ink.raise)
        };
        Self {
            dark,
            fg,
            page,
            k04: wash(settings_wash::ROW),
            k05: wash(settings_wash::KEYCAP),
            k06: wash(settings_wash::MENU_ROW),
            k08: a(0.08, 0.08),
            k15: a(0.15, 0.16),
            k3: a(0.3, 0.3),
            glyph: role(ink.glyph),
            k45: a(0.45, 0.5),
            sub: role(ink.sub),
            switch_off: role(ink.switch_off).opacity(ink.switch_off_alpha),
            heading: a(0.9, 0.9),
            nav: a(0.82, 0.8),
            btn: if dark { pole.opacity(0.1) } else { raised },
            menu: if dark {
                lift(page, ink.menu_lift)
            } else {
                raised
            },
            knob: if dark { fg } else { raised },
            chip: if dark { pole.opacity(0.16) } else { raised },
            warn: role(ink.warn),
            warn_text: role(ink.warn_text),
            unsaved: role(ink.unsaved),
            danger: role(ink.danger),
            ok: role(ink.ok),
        }
    }

    /// The hairline ring and the faint drop a raised control sits on.
    pub(crate) fn raised(&self) -> Vec<BoxShadow> {
        if self.dark {
            vec![ring(self.fg.opacity(0.1), 0.5, false)]
        } else {
            vec![
                ring(gpui::black().opacity(0.08), 0.5, false),
                drop(gpui::black().opacity(0.06), 1., 2., 0.),
            ]
        }
    }

    /// The same, a step firmer, for a hovered raised control.
    pub(crate) fn raised_hover(&self) -> Vec<BoxShadow> {
        if self.dark {
            vec![ring(self.fg.opacity(0.2), 0.5, false)]
        } else {
            vec![
                ring(gpui::black().opacity(0.16), 0.5, false),
                drop(gpui::black().opacity(0.08), 1., 2., 0.),
            ]
        }
    }

    pub(crate) fn popover(&self) -> Vec<BoxShadow> {
        if self.dark {
            vec![
                ring(self.fg.opacity(0.12), 0.5, false),
                drop(gpui::black().opacity(0.6), 16., 40., -8.),
            ]
        } else {
            vec![
                ring(gpui::black().opacity(0.12), 0.5, false),
                drop(gpui::black().opacity(0.22), 16., 40., -8.),
            ]
        }
    }

    pub(crate) fn mono(cx: &App) -> SharedString {
        cx.theme().mono_font_family.clone()
    }
}

pub(crate) fn ring(color: Hsla, width: f32, inset: bool) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(width),
        inset,
    }
}

pub(crate) fn drop(color: Hsla, y: f32, blur: f32, spread: f32) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ToggleHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// An on/off switch: ink when on, a hairline track when off.
#[derive(IntoElement)]
pub(crate) struct Switch {
    id: ElementId,
    on: bool,
    disabled: bool,
    small: bool,
    handler: Option<ToggleHandler>,
    /// The name a screen reader gives the switch, where no settings row
    /// around it already says what it is.
    label: Option<SharedString>,
    /// Scrollers to bring it into view in, inner to outer (see [`Stop`]).
    scrolls: Vec<gpui::ScrollHandle>,
}

pub(crate) fn switch(id: impl Into<ElementId>) -> Switch {
    Switch {
        id: id.into(),
        on: false,
        disabled: false,
        small: false,
        handler: None,
        label: None,
        scrolls: Vec::new(),
    }
}

impl Switch {
    /// Opts into scrolling in `outer` when the keyboard reaches it.
    pub(crate) fn in_scroll(mut self, outer: Option<gpui::ScrollHandle>) -> Self {
        self.scrolls.extend(outer);
        self
    }
    pub(crate) fn checked(mut self, on: bool) -> Self {
        self.on = on;
        self
    }
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub(crate) fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }
    pub(crate) fn on_click(mut self, f: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.handler = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tk = Tk::of(cx);
        let (w, h, knob) = if self.small {
            (26., 16., 12.)
        } else {
            (30., 18., 14.)
        };
        // A stop only where there is something to do: a handler-less or
        // disabled switch (the nav's decorative one) stays as it was.
        let key = self.id.clone();
        let interactive = self.handler.is_some() && !self.disabled;
        let scrolls = self.scrolls.clone();
        let on = self.on;
        let left = if on { w - knob - 2. } else { 2. };
        // Tests find a switch by the id the caller already gave it. Both the
        // selector string and the call are cfg(test), so a production Switch
        // builds exactly the element it did before.
        #[cfg(test)]
        let debug_selector = self.id.to_string();
        let switch = div()
            .id(self.id)
            .role(gpui::Role::Switch)
            .when_some(self.label, |d, label| d.aria_label(label))
            .aria_toggled(if on {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .flex_shrink_0()
            .relative()
            .w(px(w))
            .h(px(h))
            .rounded(px(h / 2.))
            .bg(if on { tk.fg } else { tk.switch_off })
            .when(self.disabled, |d| d.opacity(0.4))
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .left(px(left))
                    .size(px(knob))
                    .rounded_full()
                    .bg(if on { tk.page } else { tk.knob })
                    .shadow(vec![drop(gpui::black().opacity(0.2), 1., 2., 0.)]),
            )
            .when_some(self.handler.filter(|_| !self.disabled), |d, handler| {
                d.cursor_pointer()
                    .on_click(move |_, window, cx| handler(&!on, window, cx))
            });
        #[cfg(test)]
        let switch = probe(
            if on {
                "settings.switch-on"
            } else {
                "settings.switch-off"
            },
            switch,
        );
        #[cfg(test)]
        let switch = mark(switch, &debug_selector).debug_selector(move || debug_selector.clone());
        stop(key, px(h / 2.), switch)
            .live(interactive)
            .scrolls(scrolls)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BtnKind {
    /// A raised button on the page.
    Secondary,
    /// Solid ink: the one action a block exists for.
    Primary,
    /// Text that darkens on hover, for actions beside other actions.
    Link,
    /// A link in red, for the destructive one.
    Danger,
}

#[derive(IntoElement)]
pub(crate) struct Btn {
    id: ElementId,
    label: SharedString,
    kind: BtnKind,
    disabled: bool,
    dimmed: bool,
    icon: Option<SharedString>,
    size: Option<Rems>,
    announced: bool,
    min_hit: bool,
    handler: Option<ClickHandler>,
    /// Scrollers to bring it into view in, inner to outer (see [`Stop`]).
    scrolls: Vec<gpui::ScrollHandle>,
}

/// The shortest a link may be to the pointer where it stands alone in a bar
/// (a popover's footer), rather than inline beside other text.
pub(crate) const LINK_HIT_HEIGHT: Pixels = px(28.);

pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: BtnKind,
) -> Btn {
    Btn {
        id: id.into(),
        label: label.into(),
        kind,
        disabled: false,
        dimmed: false,
        icon: None,
        size: None,
        announced: false,
        min_hit: false,
        handler: None,
        scrolls: Vec::new(),
    }
}

impl Btn {
    /// Opts into scrolling in `outer` when the keyboard reaches it.
    pub(crate) fn in_scroll(mut self, outer: Option<gpui::ScrollHandle>) -> Self {
        self.scrolls.extend(outer);
        self
    }
    /// An inner scroller first, then the outer ones.
    pub(crate) fn in_inner_scroll(mut self, inner: Option<gpui::ScrollHandle>) -> Self {
        if let Some(inner) = inner {
            self.scrolls.insert(0, inner);
        }
        self
    }
    /// Report the button to assistive technology under its visible label. Off by
    /// default so buttons that have not been audited keep the tree they had.
    pub(crate) fn announced(mut self) -> Self {
        self.announced = true;
        self
    }
    /// Grows the click target to [`LINK_HIT_HEIGHT`] without moving the
    /// label, for a link alone in a bar. Off by default, so inline links keep
    /// the height of their text.
    pub(crate) fn min_hit_height(mut self) -> Self {
        self.min_hit = true;
        self
    }
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Clickable, but drawn at rest — a Save with nothing to save yet.
    pub(crate) fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }
    pub(crate) fn icon(mut self, path: impl Into<SharedString>) -> Self {
        self.icon = Some(path.into());
        self
    }
    pub(crate) fn text_size(mut self, size: Rems) -> Self {
        self.size = Some(size);
        self
    }
    pub(crate) fn on_click(
        mut self,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handler = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Btn {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tk = Tk::of(cx);
        let kind = self.kind;
        // A stop only where there is something to do: no handler, or disabled,
        // and the button is drawn exactly as it was. A dimmed one still acts,
        // so it is a stop.
        let key = self.id.clone();
        let interactive = self.handler.is_some() && !self.disabled;
        let scrolls = self.scrolls.clone();
        let radius = match kind {
            BtnKind::Secondary | BtnKind::Primary => px(6.),
            BtnKind::Link | BtnKind::Danger => px(4.),
        };
        // A primary with nothing to do yet — disabled, or dimmed with nothing
        // to save — falls to the faint fill with secondary text, the way v4's
        // Commit button and the dialogs' Create do. Faded to half opacity it
        // stayed a black button that still looked like the thing to press.
        let resting_primary = kind == BtnKind::Primary && (self.disabled || self.dimmed);
        // Tests find a button by the id the caller already gave it, as with
        // `Switch`; a production button builds the element it did before.
        #[cfg(test)]
        let debug_selector = self.id.to_string();
        #[cfg(test)]
        crate::ui::paint_probe::note_label(&self.id.to_string(), &self.label);
        let base = h_flex()
            .id(self.id)
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .whitespace_nowrap();
        let el = match kind {
            BtnKind::Secondary => base
                .h(px(26.))
                .px(px(12.))
                .when(self.icon.is_some(), |b| b.pl(px(9.)).pr(px(11.)))
                .rounded(px(6.))
                .bg(tk.btn)
                .shadow(tk.raised())
                .text_size(self.size.unwrap_or(fs(12.5)))
                .text_color(tk.fg)
                .when(!self.disabled, |b| {
                    b.hover(move |s| s.shadow(tk.raised_hover()))
                        .active(move |s| s.bg(tk.k05))
                }),
            BtnKind::Primary => base
                .h(px(26.))
                .px(px(12.))
                .rounded(px(6.))
                .bg(if resting_primary { tk.k05 } else { tk.fg })
                .text_size(self.size.unwrap_or(fs(12.5)))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if resting_primary { tk.sub } else { tk.page })
                .when(!resting_primary, |b| b.hover(|s| s.opacity(0.88)))
                .when(resting_primary && !self.disabled, |b| {
                    b.hover(move |s| s.bg(tk.k08))
                }),
            BtnKind::Link | BtnKind::Danger => base
                .text_size(self.size.unwrap_or(fs(12.)))
                .text_color(if kind == BtnKind::Danger {
                    tk.danger
                } else {
                    tk.sub
                })
                .when(!self.disabled, |b| {
                    b.hover(move |s| {
                        s.text_color(if kind == BtnKind::Danger {
                            tk.danger
                        } else {
                            tk.fg
                        })
                    })
                }),
        };
        let fades = kind != BtnKind::Primary;
        let button = el
            // Positioned only where the ring layer lies over it: an inert
            // button stays exactly the element it was.
            .when(interactive, |b| b.relative())
            .when(self.min_hit, |b| b.min_h(LINK_HIT_HEIGHT))
            .when(self.announced, |b| {
                b.role(gpui::Role::Button).aria_label(self.label.clone())
            })
            .when(fades && self.disabled, |b| b.opacity(0.45))
            .when(fades && self.dimmed && !self.disabled, |b| b.opacity(0.55))
            .when_some(self.icon, |b, path| {
                b.child(Icon::empty().path(path).size(px(10.)).text_color(tk.sub))
            })
            .child(self.label)
            .when_some(self.handler.filter(|_| !self.disabled), |b, handler| {
                b.cursor_pointer()
                    .on_click(move |ev, window, cx| handler(ev, window, cx))
            });
        #[cfg(test)]
        let button = probe(
            match kind {
                BtnKind::Secondary => "settings.btn-secondary",
                BtnKind::Primary => "settings.btn-primary",
                BtnKind::Link => "settings.btn-link",
                BtnKind::Danger => "settings.btn-danger",
            },
            button,
        );
        #[cfg(test)]
        let button = mark(button, &debug_selector).debug_selector(move || debug_selector.clone());
        stop(key, radius, button).live(interactive).scrolls(scrolls)
    }
}

/// A key on a keycap: `⌘`, `T`, `Space`.
pub(crate) fn keycap(key: impl Into<SharedString>, tk: &Tk) -> Div {
    let keycap = div()
        .flex_shrink_0()
        .min_w(px(20.))
        .h(px(20.))
        .px(px(5.))
        .rounded(px(4.))
        .bg(tk.k05)
        .shadow(vec![BoxShadow {
            color: tk.k15,
            offset: point(px(0.), px(-0.5)),
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: true,
        }])
        .flex()
        .items_center()
        .justify_center()
        .text_size(fs(11.5))
        .text_color(tk.sub)
        .child(key.into());
    #[cfg(test)]
    let keycap = probe("settings.keycap", keycap);
    keycap
}

/// A search glass the size the design draws it beside a field: one SVG, painted
/// in the cached decorative-mark ink.
pub(crate) fn search_glass(size: f32, tk: &Tk) -> gpui::Svg {
    let glass = gpui::svg()
        .flex_none()
        .size(px(size))
        .path("icons/settings/search.svg")
        .text_color(tk.glyph);
    #[cfg(test)]
    let glass = probe("settings.search-glass", glass);
    glass
}

/// A hairline: one device pixel. The design draws field edges at half a
/// point, which is exactly one pixel on a Retina panel — and on a 1x display
/// half a pixel at 15% ink is anti-aliased to nothing, so the shell and proxy
/// fields read as loose monospace text beside the boxed dropdowns.
///
/// Read from the scale the settings window last rendered at: the page's
/// controls are built from `cx` alone, and threading a `Window` through every
/// row to reach two fields would be a lot of signature for one number. A
/// thread-local rather than a global, because setting a global from `render`
/// queues an observer effect on every frame.
pub(crate) fn hairline() -> f32 {
    (1. / SCALE.with(std::cell::Cell::get)).max(0.5)
}

/// Record the scale the page is being drawn at, once per frame.
pub(crate) fn note_scale(window: &Window) {
    SCALE.with(|s| s.set(window.scale_factor().max(1.)));
}

thread_local! {
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(2.) };
}

/// A search field with a hairline ring, for filtering a list on the page.
pub(crate) fn search_field(input: &gpui::Entity<InputState>, tk: &Tk) -> Div {
    h_flex()
        .h(px(26.))
        .px(px(8.))
        .gap(px(6.))
        .items_center()
        .rounded(px(6.))
        .shadow(vec![ring(tk.k15, hairline(), true)])
        .child(search_glass(10., tk))
        .child(
            // On the input, not around it: `Input` sets its own `text_sm`
            // and only a size given to it directly outranks that.
            div().flex_1().min_w_0().child(
                Input::new(input)
                    .appearance(false)
                    .px_0()
                    .py_0()
                    .h(px(24.))
                    .text_size(fs(12.5)),
            ),
        )
}

/// A text field the way the page draws one: a hairline ring on the page, in
/// the terminal's monospace, with a firmer ring while it has focus.
pub(crate) fn text_field(
    input: &gpui::Entity<InputState>,
    focused: bool,
    invalid: bool,
    tk: &Tk,
    cx: &App,
) -> Div {
    field_box(input, focused, invalid, false, tk, cx)
}

/// A [`text_field`] for a masked input, with the eye that reveals it.
pub(crate) fn secret_field(
    input: &gpui::Entity<InputState>,
    focused: bool,
    tk: &Tk,
    cx: &App,
) -> Div {
    field_box(input, focused, false, true, tk, cx)
}

fn field_box(
    input: &gpui::Entity<InputState>,
    focused: bool,
    invalid: bool,
    reveal: bool,
    tk: &Tk,
    cx: &App,
) -> Div {
    let ring_color = if invalid {
        tk.danger
    } else if focused {
        tk.k45
    } else {
        tk.k15
    };
    // The field's keyboard focus indication is the shared ring too, drawn inside
    // the box over the Input's own handle (no new tab stop), alongside the
    // field's own inset ring, which stays.
    let handle = input.read(cx).focus_handle(cx);
    let field = div()
        .relative()
        .h(px(26.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .shadow(vec![ring(
            ring_color,
            if focused || invalid { 1. } else { hairline() },
            true,
        )])
        .font_family(Tk::mono(cx))
        .text_color(tk.fg)
        .child(
            div().flex_1().min_w_0().child(
                Input::new(input)
                    .appearance(false)
                    .px_0()
                    .py_0()
                    .h(px(24.))
                    // The input's own `text_sm` beat the 12px set on this
                    // box, so the value stood a size above every dropdown
                    // label in the same column.
                    .text_size(fs(12.))
                    .when(reveal, |i| i.mask_toggle()),
            ),
        )
        .child(ring_layer(
            &handle,
            RingPlacement::Inside,
            px(6.),
            false,
            || format!("settings-field-{}", input.entity_id().as_u64()),
        ));
    // Tests find one field among many by its Input entity: the box's measured
    // bounds and its own painted shadow, observed after the shadow is applied.
    #[cfg(test)]
    let field = {
        let id = input.entity_id().as_u64();
        let mut field = field;
        crate::ui::paint_probe::note(&format!("settings.field-{id}"), &mut field);
        field.debug_selector(move || format!("settings-field-{id}"))
    };
    field
}

/// A group's heading and, when it has one, the sentence under it.
pub(crate) fn section_head(title: &str, desc: Option<String>, tk: &Tk) -> Div {
    v_flex()
        .gap(px(3.))
        .pb(px(8.))
        .child(
            div()
                .text_size(fs(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tk.heading)
                .child(title.to_string()),
        )
        .when_some(desc.filter(|d| !d.is_empty()), |col, desc| {
            let desc = div()
                .text_size(fs(12.))
                .line_height(px(17.))
                .text_color(tk.sub)
                .child(desc);
            #[cfg(test)]
            let desc = probe("settings.section-desc", desc);
            col.child(desc)
        })
}

/// The width of a control in the page's right-hand column — dropdown or text
/// field alike. The column used to size each dropdown to its value (anything
/// from 148 to 260) beside fields fixed at 180, so its left edge zig-zagged
/// down the page. One width, and a value too long for it elides. Only fields
/// that hold a path or a command line are wider, because those are unreadable
/// cut short.
pub(crate) const CONTROL_W: f32 = 180.;

/// The trigger of a dropdown: a raised button with the current value, an
/// optional mark before it, and a chevron, [`CONTROL_W`] wide.
///
/// `disabled` is the settings kit's disabled look (the switch's and button's:
/// faded, no hover lift, no pointer). The caller owns the behavior and must
/// not attach a click handler to a disabled trigger.
pub(crate) fn select_trigger(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    leading: Option<AnyElement>,
    open: bool,
    disabled: bool,
    tk: &Tk,
) -> Stateful<Div> {
    #[cfg(test)]
    let id: ElementId = id.into();
    #[cfg(test)]
    let debug_selector = id.to_string();
    let trigger = h_flex()
        .id(id)
        .h(px(26.))
        .w(px(CONTROL_W))
        .flex_shrink_0()
        .pl(px(if leading.is_some() { 8. } else { 10. }))
        .pr(px(7.))
        .gap(px(8.))
        .items_center()
        .rounded(px(6.))
        .bg(if open && !disabled { tk.k05 } else { tk.btn })
        .shadow(tk.raised())
        .when(!disabled, |d| {
            d.hover(move |s| s.shadow(tk.raised_hover()))
                .cursor_pointer()
        })
        .when(disabled, |d| d.opacity(0.45).cursor_default())
        .text_size(fs(12.5))
        .text_color(tk.fg)
        .whitespace_nowrap()
        .children(leading)
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .child(chevron_down(tk));
    #[cfg(test)]
    let trigger = trigger.debug_selector(move || debug_selector.clone());
    trigger
}

pub(crate) fn chevron_down(tk: &Tk) -> Icon {
    Icon::empty()
        .path("icons/settings/chevron-down.svg")
        .size(px(10.))
        .text_color(tk.sub)
}

pub(crate) fn check_mark(visible: bool, tk: &Tk) -> Div {
    div().size(px(12.)).flex_shrink_0().when(visible, |d| {
        d.child(
            Icon::empty()
                .path("icons/settings/check.svg")
                .size(px(12.))
                .text_color(tk.fg),
        )
    })
}

/// A popover's surface.
pub(crate) fn menu_panel(tk: &Tk) -> Div {
    let panel = v_flex()
        .p(px(4.))
        .gap(px(1.))
        .rounded(crate::ui::rounding::POPOVER_RADIUS)
        .bg(tk.menu)
        .shadow(tk.popover())
        .text_size(fs(12.5))
        .text_color(tk.fg);
    #[cfg(test)]
    let panel = probe("settings.menu-panel", panel);
    panel
}

/// One pickable row in a popover menu. Its 6px corners sit concentric inside
/// the panel's 10px ones, across the panel's 4px padding.
pub(crate) fn menu_row(id: impl Into<ElementId>, highlighted: bool, tk: &Tk) -> Stateful<Div> {
    let row = h_flex()
        .id(id)
        .flex_shrink_0()
        .h(px(26.))
        .pl(px(6.))
        .pr(px(12.))
        .gap(px(6.))
        .items_center()
        .rounded(px(6.))
        .whitespace_nowrap()
        .cursor_pointer()
        .when(highlighted, |r| r.bg(tk.k06))
        .hover(move |s| s.bg(tk.k06));
    #[cfg(test)]
    let row = probe("settings.menu-row", row);
    row
}

pub(crate) fn menu_heading(label: impl Into<SharedString>, tk: &Tk) -> Div {
    div()
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .text_size(fs(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(tk.sub)
        .child(label.into())
}

pub(crate) fn menu_separator(tk: &Tk) -> Div {
    div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(tk.k08)
}

/// Where a trigger's popover hangs: under the trigger, its right edge on the
/// trigger's (or its left on the trigger's), painted over the rest of the page
/// and pushed back inside the window when it would run off it.
pub(crate) fn popover_below(align_right: bool, gap: f32, content: impl IntoElement) -> Div {
    let anchor = if align_right {
        gpui::Anchor::TopRight
    } else {
        gpui::Anchor::TopLeft
    };
    div()
        .absolute()
        .top(px(26. + gap))
        .when(align_right, |d| d.right_0())
        .when(!align_right, |d| d.left_0())
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .occlude()
                            // A click inside the popover is not a click on the
                            // page under it, which would close it.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(content),
                    ),
            )
            .with_priority(2),
        )
}

/// The stepper: − value +, one raised control with hairline dividers.
pub(crate) struct Stepper {
    pub value: String,
    pub unit: String,
    pub can_dec: bool,
    pub can_inc: bool,
}

pub(crate) fn stepper(
    id: &'static str,
    s: Stepper,
    tk: &Tk,
    on_dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    scroll: Option<gpui::ScrollHandle>,
) -> AnyElement {
    let end = |suffix: &'static str, glyph: &'static str, enabled: bool| {
        div()
            .id(SharedString::from(format!("{id}-{suffix}")))
            .w(px(28.))
            .h(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(fs(13.))
            .text_color(tk.sub)
            .when(!enabled, |d| d.opacity(0.35))
            .when(enabled, |d| {
                d.cursor_pointer().active(move |s| s.bg(tk.k05))
            })
            .child(glyph)
    };
    let divider = || div().h(px(14.)).w(px(1.)).bg(tk.k15);
    // One stop for the whole control, so one Tab stop and one ring concentric
    // with the raised pill (a ring per end would meet at the divider with a
    // half-square corner). The same handlers serve the mouse ends and the keys.
    let (can_dec, can_inc) = (s.can_dec, s.can_inc);
    let on_dec: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)> = Rc::new(on_dec);
    let on_inc: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)> = Rc::new(on_inc);
    let (key_dec, key_inc) = (on_dec.clone(), on_inc.clone());
    let (end_dec, end_inc) = (on_dec, on_inc);
    let control = h_flex()
        .id(id)
        .relative()
        .h(px(26.))
        .items_center()
        .rounded(px(6.))
        .bg(tk.btn)
        .shadow(tk.raised())
        .child(
            end("dec", "−", s.can_dec)
                .rounded_l(px(6.))
                .when(s.can_dec, |d| {
                    d.on_click(move |ev, window, cx| end_dec(ev, window, cx))
                }),
        )
        .child(divider())
        .child(
            h_flex()
                .min_w(px(52.))
                .px(px(6.))
                .justify_center()
                .text_size(fs(12.5))
                .text_color(tk.fg)
                .child(s.value)
                .child(div().text_color(tk.sub).child(s.unit)),
        )
        .child(divider())
        .child(
            end("inc", "+", s.can_inc)
                .rounded_r(px(6.))
                .when(s.can_inc, |d| {
                    d.on_click(move |ev, window, cx| end_inc(ev, window, cx))
                }),
        );
    #[cfg(test)]
    let control = control.debug_selector(|| id.to_string());
    stop(id, px(6.), control)
        .in_scroll(scroll)
        .on_key(move |ev, window, cx| {
            if !plain_key(ev) {
                return;
            }
            // Left/Down step down, Right/Up step up, each only where its end is
            // live, through the same handlers the ends call (a keyboard click).
            let handler = match ev.keystroke.key.as_str() {
                "left" | "down" if can_dec => &key_dec,
                "right" | "up" if can_inc => &key_inc,
                _ => return,
            };
            handler(&ClickEvent::default(), window, cx);
            cx.stop_propagation();
        })
        .into_any_element()
}

/// How far one arrow key moves a slider, as a fraction of its track.
const SLIDER_KEY_STEP: f32 = 0.05;

/// What a slider is being dragged by; the id tells two sliders apart.
#[derive(Clone)]
pub(crate) struct SliderDrag(pub SharedString);

impl Render for SliderDrag {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A thin track with an ink fill and a raised knob, 148 wide, with its value
/// read out beside it. `on_set` takes the fraction along the track the pointer
/// is at; `on_release` is where a drag's value gets written down for good.
pub(crate) fn slider(
    id: impl Into<SharedString>,
    fraction: f32,
    readout: String,
    tk: &Tk,
    on_set: Rc<dyn Fn(f32, &mut Window, &mut App)>,
    on_release: Rc<dyn Fn(&mut Window, &mut App)>,
    scroll: Option<gpui::ScrollHandle>,
) -> AnyElement {
    let id: SharedString = id.into();
    let f = fraction.clamp(0., 1.);
    const W: f32 = 148.;
    let frac_at = |bounds: gpui::Bounds<gpui::Pixels>, x: gpui::Pixels| -> f32 {
        let w = bounds.size.width.as_f32().max(1.);
        ((x - bounds.origin.x).as_f32() / w).clamp(0., 1.)
    };
    let set_down = on_set.clone();
    let set_move = on_set;
    let drag_id = id.clone();
    let release_up = on_release.clone();
    let key_set = set_down.clone();
    let key_release = release_up.clone();
    let release_out = on_release;
    let bounds = Rc::new(std::cell::Cell::new(gpui::Bounds::<gpui::Pixels>::default()));
    let track = div()
        .id(ElementId::Name(id.clone()))
        .relative()
        .w(px(W))
        .h(px(18.))
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(8.))
                .h(px(2.))
                .rounded(px(1.))
                .bg(tk.k15),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(8.))
                .h(px(2.))
                .w(px(W * f))
                .rounded(px(1.))
                .bg(tk.fg),
        )
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(W * f - 7.))
                .size(px(14.))
                .rounded_full()
                .bg(tk.knob)
                .shadow(vec![
                    ring(gpui::black().opacity(0.18), 0.5, false),
                    drop(gpui::black().opacity(0.18), 1., 3., 0.),
                ]),
        )
        .child({
            // The track's bounds for a click that never becomes a
            // drag: a mouse-down event carries a position but not the
            // box it landed in.
            let bounds = bounds.clone();
            gpui::canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .size_full()
        })
        .on_mouse_down(MouseButton::Left, {
            let bounds = bounds.clone();
            move |ev: &gpui::MouseDownEvent, window, cx| {
                set_down(frac_at(bounds.get(), ev.position.x), window, cx);
            }
        })
        .on_drag(SliderDrag(drag_id), |drag, _, _, cx| {
            cx.new(|_| drag.clone())
        })
        .on_drag_move({
            let id = id.clone();
            move |ev: &gpui::DragMoveEvent<SliderDrag>, window, cx| {
                if ev.drag(cx).0 != id {
                    return;
                }
                set_move(frac_at(ev.bounds, ev.event.position.x), window, cx);
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            release_up(window, cx)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
            release_out(window, cx)
        });
    #[cfg(test)]
    let track = track.debug_selector({
        let selector = id.to_string();
        move || selector
    });
    // The track is the stop (the readout beside it is not). A key press is one
    // drag-and-release: Left/Right move the fraction by a fixed step, Home/End
    // go to the ends; each press that changes the value calls `on_set` then
    // `on_release` once, like a drag, and each caller keeps its own mapping. The
    // knob overhangs the track by 7px at both ends, so the ring (outside the
    // track box) crosses it there.
    let track = stop(ElementId::Name(id.clone()), px(9.), track)
        .in_scroll(scroll)
        .on_key(move |ev, window, cx| {
            if !plain_key(ev) {
                return;
            }
            let next = match ev.keystroke.key.as_str() {
                "left" => (f - SLIDER_KEY_STEP).max(0.),
                "right" => (f + SLIDER_KEY_STEP).min(1.),
                "home" => 0.,
                "end" => 1.,
                _ => return,
            };
            cx.stop_propagation();
            if (next - f).abs() > f32::EPSILON {
                key_set(next, window, cx);
                key_release(window, cx);
            }
        });
    h_flex()
        .gap(px(8.))
        .items_center()
        .child(track)
        .child(
            div()
                .w(px(40.))
                .flex_shrink_0()
                .text_right()
                .whitespace_nowrap()
                .text_size(fs(12.5))
                .text_color(tk.sub)
                .child(readout),
        )
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod test_support {
    use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::app::Tty59App;

    /// A real `Tty59App` in a test window, themed through `apply_theme` the
    /// way the app is, for tests that drive the settings page's own render.
    pub(crate) fn harness(cx: &mut TestAppContext) -> (Entity<Tty59App>, VisualTestContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| Tty59App::with_session(None, Some(Session::default()), window, cx));
            gpui_component::Root::new(app, window, cx)
        });
        cx.background_executor.run_until_parked();
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<Tty59App>()
                    .unwrap_or_else(|_| panic!("window root wraps a Tty59App"))
            })
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (app, vcx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `#[gpui::test]` expands a bare `drop(..)`; the glob above would bring in
    // this module's own shadow-casting `drop` helper instead of std's.
    use crate::core::config::{Config, CoreConfig};
    use crate::ui::presets::{self, contrast};
    use gpui::TestAppContext;
    use std::mem::drop;

    fn pack(c: Hsla) -> u32 {
        let rgb = crate::terminal::palette::hsla_to_rgb(c);
        (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32
    }

    /// `theme::semantics`, `settings_ink` and `chrome_ink` panic without a
    /// theme and have no fallback, so a window must be built after
    /// `apply_theme` has installed all three. Nothing is cached before the app
    /// builds its view; every role is there once it has, and the settings page
    /// draws from them on its first frame.
    #[gpui::test]
    fn a_window_is_built_after_the_theme_and_finds_every_cached_role(cx: &mut TestAppContext) {
        use crate::core::session::Session;
        use crate::ui::app::Tty59App;
        use gpui::{AppContext as _, VisualTestContext};

        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(Config::default());
            crate::ui::keymap::init(cx);
            assert!(!cx.has_global::<presets::Semantics>());
            assert!(!cx.has_global::<presets::SettingsInk>());
            assert!(!cx.has_global::<presets::ChromeInk>());
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| Tty59App::with_session(None, Some(Session::default()), window, cx));
            assert!(cx.has_global::<presets::Semantics>());
            assert!(cx.has_global::<presets::SettingsInk>());
            assert!(cx.has_global::<presets::ChromeInk>());
            gpui_component::Root::new(app, window, cx)
        });
        cx.background_executor.run_until_parked();
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<Tty59App>()
                    .unwrap_or_else(|_| panic!("window root wraps a Tty59App"))
            })
            .unwrap();
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_settings_section(crate::ui::settings::SettingsSection::General, window, cx);
        });
        vcx.run_until_parked();
    }

    /// `Tk` paints text, off tracks and status colours from what `apply_theme`
    /// cached, and those colours clear their floors on the washes `Tk` itself
    /// lays down: the page, hovered rows, keycaps, menu rows and the nav
    /// rail's rungs. Measured on the painted values, not on the derivation's
    /// own list, with each layer rounded to eight bits the way the derivation
    /// rounds it, and no tolerance.
    #[gpui::test]
    fn tk_reads_the_cached_roles_and_they_clear_their_floors_on_the_painted_fills(
        cx: &mut TestAppContext,
    ) {
        // WCAG AA for text; a light off track is a mark, held to 3:1 below.
        // A dark one is the owner's faint wash; its knob, a non-text mark, is held to 3:1.
        const TEXT_FLOOR: f32 = 4.5;
        crate::core::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_component::init(cx);
            // A crossover palette too, whose washes and lifts invert.
            let crossover = presets::generated_theme_grid()
                .into_iter()
                .find(|t| t.background_color() == 0x777777)
                .unwrap();
            let crossover_id = crossover.id.clone();
            let mut themes = presets::builtins();
            themes.push(crossover);
            cx.set_global(presets::Themes(themes));
            for id in [
                "light",
                "dark",
                "catppuccin_latte",
                "rose_pine_dawn",
                "one_dark_pro",
                crossover_id.as_str(),
            ] {
                cx.set_global(Config(CoreConfig {
                    theme_follow_system: false,
                    theme_preset: id.into(),
                    ..Default::default()
                }));
                crate::ui::theme::apply_theme(None, cx);
                let tk = Tk::of(cx);
                let ink = crate::ui::theme::settings_ink(cx);
                let rail = cx.global::<presets::Surfaces>().rail;

                assert_eq!(pack(tk.sub), ink.sub, "{id}: sub is the cached role");
                assert_eq!(pack(tk.switch_off), ink.switch_off, "{id}: switch_off");
                assert_eq!(pack(tk.danger), ink.danger, "{id}: danger");
                assert_eq!(pack(tk.warn_text), ink.warn_text, "{id}: warn_text");
                assert_eq!(pack(tk.unsaved), ink.unsaved, "{id}: unsaved");
                assert_eq!(pack(tk.warn), ink.warn, "{id}: warn");
                assert_eq!(pack(tk.ok), ink.ok, "{id}: ok");
                assert_eq!(pack(tk.glyph), ink.glyph, "{id}: glyph is the cached role");
                // The washes step toward the cached pole, not the foreground.
                let pole = crate::ui::theme::fill_pole(cx);
                for (name, wash) in [("k04", tk.k04), ("k05", tk.k05), ("k06", tk.k06)] {
                    assert_eq!(pack(wash), pole, "{id}: {name} steps toward the pole");
                }
                if id == crossover_id {
                    assert_eq!(pole, 0xffffff, "{id}: the crossover pole is inverted");
                }

                // Each layer is quantized to eight bits before the next is drawn over it.
                let over = |fill: u32, wash: Hsla| pack(Hsla::from(gpui::rgb(fill)).blend(wash));
                let text_fills = [
                    pack(tk.page),
                    over(pack(tk.page), tk.k04),
                    over(pack(tk.page), tk.k05),
                    pack(tk.menu),
                    over(pack(tk.menu), tk.k06),
                    rail.base,
                    rail.hover,
                    rail.selected,
                    over(rail.base, tk.k05),
                ];
                for fill in text_fills {
                    assert!(
                        contrast(pack(tk.sub), fill) >= TEXT_FLOOR,
                        "{id}: sub {:#08x} is {:.2}:1 on {fill:#08x}",
                        pack(tk.sub),
                        contrast(pack(tk.sub), fill)
                    );
                }
                let mark_fills = [
                    pack(tk.page),
                    over(pack(tk.page), tk.k04),
                    over(pack(tk.page), tk.k05),
                    rail.base,
                    over(rail.base, tk.k05),
                ];
                // Error, Unsaved and Live text on the fills they are painted on:
                // the page, a hovered or open row, a dropdown and its row.
                let status_fills = [
                    pack(tk.page),
                    over(pack(tk.page), tk.k04),
                    pack(tk.menu),
                    over(pack(tk.menu), tk.k06),
                ];
                // Unsaved is painted only on the open host row (the page under the k04 wash) and is a
                // small role: it holds the literal 5.0 there, and the 4.5 floor on it too.
                let host_row = over(pack(tk.page), tk.k04);
                assert!(
                    contrast(pack(tk.unsaved), host_row) >= 5.0,
                    "{id}: unsaved {:#08x} is {:.2}:1 on the open host row {host_row:#08x}, under 5.0",
                    pack(tk.unsaved),
                    contrast(pack(tk.unsaved), host_row)
                );
                // The general warning text (agent hooks Unavailable) is painted on the page.
                assert!(
                    contrast(pack(tk.warn_text), pack(tk.page)) >= TEXT_FLOOR,
                    "{id}: warn_text {:#08x} is {:.2}:1 on the page",
                    pack(tk.warn_text),
                    contrast(pack(tk.warn_text), pack(tk.page))
                );
                for (what, text) in [("danger", tk.danger), ("ok", tk.ok)] {
                    for fill in status_fills {
                        assert!(
                            contrast(pack(text), fill) >= TEXT_FLOOR,
                            "{id}: {what} {:#08x} is {:.2}:1 on {fill:#08x}",
                            pack(text),
                            contrast(pack(text), fill)
                        );
                    }
                }
                // The search glass: the page, the nav rail's search well, and
                // the open dropdown's own fill.
                for fill in [
                    pack(tk.page),
                    over(pack(tk.page), tk.k04),
                    rail.base,
                    over(rail.base, tk.k04),
                    pack(tk.menu),
                ] {
                    assert!(
                        contrast(pack(tk.glyph), fill) >= 3.,
                        "{id}: glyph {:#08x} is {:.2}:1 on {fill:#08x}",
                        pack(tk.glyph),
                        contrast(pack(tk.glyph), fill)
                    );
                }
                // Dark fills step toward the pole: the button fill lifts off a
                // dark page when the pole is light, and sinks into it when not.
                if tk.dark {
                    let page = pack(tk.page);
                    assert_eq!(
                        presets::is_lighter(over(pack(tk.page), tk.btn), page),
                        presets::is_lighter(pole, page),
                        "{id}: button fill follows the pole"
                    );
                }
                if tk.dark {
                    // Owner exception to the 3:1 mark floor: the dark off track is
                    // exactly what `k15` always was, the foreground at 16%,
                    // translucent over whatever it sits on. Not a pole wash, not
                    // floored, not an opaque composite; the knob carries the state.
                    assert_eq!(tk.switch_off.a, 0.16, "{id}: dark off track opacity");
                    assert_eq!(tk.switch_off.a, tk.k15.a, "{id}: the old k15 opacity");
                    assert_eq!(
                        pack(tk.switch_off),
                        pack(tk.k15),
                        "{id}: the track's ink is the old k15's foreground"
                    );
                    assert_eq!(
                        pack(tk.switch_off),
                        pack(tk.fg),
                        "{id}: the track's ink is the foreground"
                    );
                    // The painted knob is `tk.knob` (the foreground), a non-text mark
                    // held to 3:1, seen against the page and against the track
                    // composited over the page and over a hovered or open row.
                    let page = pack(tk.page);
                    let row = over(page, tk.k04);
                    for (what, fill) in [
                        ("page", page),
                        ("track on the page", over(page, tk.switch_off)),
                        ("track on a row", over(row, tk.switch_off)),
                    ] {
                        assert!(
                            contrast(pack(tk.knob), fill) >= 3.,
                            "{id}: dark off knob {:#08x} is {:.2}:1 on the {what} {fill:#08x}",
                            pack(tk.knob),
                            contrast(pack(tk.knob), fill)
                        );
                    }
                } else {
                    assert_eq!(tk.switch_off.a, 1., "{id}: the light off track is opaque");
                    for fill in mark_fills {
                        assert!(
                            contrast(pack(tk.switch_off), fill) >= 3.,
                            "{id}: off track {:#08x} is {:.2}:1 on {fill:#08x}",
                            pack(tk.switch_off),
                            contrast(pack(tk.switch_off), fill)
                        );
                    }
                }
            }
        });
    }
}
