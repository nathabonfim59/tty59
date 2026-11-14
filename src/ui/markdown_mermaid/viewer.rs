//! The full-screen diagram viewer: one Mermaid diagram blown up over its
//! preview, with its own zoom and pan.
//!
//! The plugin that draws diagrams has no handle to the code editor, but it
//! does know its [`PreviewIdentity`] — the buffer and the editor group — so
//! the editor and the plugin meet through an app-global registry of open
//! viewers instead of a callback threaded through the stable plugin
//! registration. The expand button on a rendered diagram opens one; the
//! editor's preview arm layers [`render`] over the preview when a viewer is
//! open for its identity. The preview stays mounted underneath, so its
//! scroll position survives with nothing extra to do.
//!
//! Everything here is session state: nothing is persisted, and the viewer
//! starts fitted every time it opens.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, CursorStyle, EntityId, FocusHandle, Global, ImageSource, Pixels,
    Point, Task, Window, div, img, px,
};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, v_flex};
use tty59_mermaid::MermaidTheme;

use super::PreviewIdentity;
use super::render::raster_to_image;
use super::viewport::{self, Viewport};
use crate::core::actions::{DecreaseFontSize, IncreaseFontSize, ResetFontSize};
use crate::core::config::Config;
use crate::ui::i18n::{L10nKey, t};
use crate::ui::zoom::{SCROLL_GESTURE_IDLE, zoom_wheel};

/// How long the zoom must settle before a new raster is requested: one
/// raster per √2 bucket is the plan, and paying it for every wheel event of
/// a flick would render the same bucket a dozen times on the way past.
const SETTLE: Duration = Duration::from_millis(120);

/// The zoom the font-size actions send: coarse on purpose — they are the
/// keyboard, not the gesture.
const ACTION_ZOOM: f32 = 1.25;

/// One open viewer, keyed by the preview it was opened from.
pub(crate) struct Viewer {
    /// The buffer the identity was built from, so closing the buffer can
    /// find every entry of its — the identity itself is a one-way mix.
    buffer: EntityId,
    /// The editor group, for the per-frame sweep of what is on screen.
    slot: usize,
    block_id: u64,
    /// Refreshed from the block on every preview pass, so an edit in the
    /// source re-renders the diagram the viewer is showing.
    source: String,
    view: Viewport,
    /// The natural (CSS) size, known once any raster has landed. Fit waits
    /// for it: there is nothing sensible to scale before that.
    natural: Option<viewport::Size>,
    /// The area the viewer last laid out in, for re-fit on resize.
    area: Option<viewport::Size>,
    /// Re-fit on resize while the user has not zoomed or panned.
    fitted: bool,
    raster: Option<Raster>,
    /// The image the current one replaced, held until the frame after the
    /// swap (its atlas tiles may still be on screen) and dropped then.
    retired: Option<Raster>,
    in_flight: Option<InFlight>,
    /// Bumped by every zoom that wants a raster and by every source or
    /// theme change, so a completion nobody is waiting for is discarded.
    generation: u64,
    /// A failed render's one-line message, shown over the last good image.
    error: Option<String>,
    /// The one window this viewer is drawn in, noted by the bookkeeping
    /// canvas each frame: fresh rasters have no window of their own until
    /// one draws them.
    drawing_window: Option<gpui::AnyWindowHandle>,
    focus: FocusHandle,
    drag: Option<Point<Pixels>>,
    /// The wheel latch, in the terminal's shape: a gesture answers "pan or
    /// zoom?" once and keeps the answer until the stream dies.
    gesture_zoom: Option<bool>,
    gesture_until: Option<Instant>,
}

/// The image currently on screen, with everything needed to replace and
/// eventually release it.
struct Raster {
    image: Arc<gpui::RenderImage>,
    /// The CSS size this raster represents (device px ÷ its own scale).
    natural: viewport::Size,
    /// The device scale it was rendered at — the √2 bucket, not the exact
    /// viewer zoom.
    bucket: f32,
    /// The palette it was drawn with; a theme change re-renders.
    theme: MermaidTheme,
    /// The window whose atlas holds its tiles, once one has drawn it.
    window: Option<gpui::AnyWindowHandle>,
    /// Whether the viewer owns this image. The seed is the preview cache's
    /// entry — its atlas tiles are the cache's to manage, and dropping them
    /// here would pull the image out from under the inline preview — so
    /// only rasters the viewer rendered itself are ever released.
    owned: bool,
}

struct InFlight {
    bucket: f32,
    theme: MermaidTheme,
    /// Held so the task lives as long as the viewer wants it; dropping the
    /// viewer cancels a render nothing is waiting for.
    _task: Task<()>,
}

/// The registry of open viewers, at most one per preview identity.
#[derive(Default)]
pub(crate) struct DiagramViewers {
    map: HashMap<PreviewIdentity, Viewer>,
    /// The block ids one preview pass has witnessed since the overlay last
    /// looked. Taken (emptied) by the overlay each frame: a non-empty list
    /// that lacks the viewer's block means the fence that made the diagram
    /// is gone; an empty list means the preview's view was cached this
    /// frame and nothing was witnessed — which says nothing.
    witnessed: HashMap<PreviewIdentity, Vec<u64>>,
}

impl Global for DiagramViewers {}

/// A block was drawn for `identity` — one line of that preview's witness
/// list. Also refreshes an open viewer's copy of the source, so an edit
/// re-renders the diagram it is showing.
pub(crate) fn witness(identity: PreviewIdentity, block_id: u64, source: &str, cx: &mut App) {
    if !cx.has_global::<DiagramViewers>() {
        return;
    }
    let viewers = cx.global_mut::<DiagramViewers>();
    if !viewers.map.contains_key(&identity) {
        return;
    }
    viewers
        .witnessed
        .entry(identity)
        .or_default()
        .push(block_id);
    let viewer = viewers.map.get_mut(&identity).expect("checked above");
    if viewer.source != source {
        viewer.source = source.to_string();
        // An edited diagram re-renders at the bucket the viewport is at;
        // the viewport itself is kept — the reader's place is in the
        // document, not in the pixels.
        viewer.generation += 1;
        viewer.error = None;
    }
}

/// Whether a viewer is open for `identity` — the editor's own wheel zoom
/// stands down when one is, so the gesture belongs to the diagram.
pub(crate) fn is_open(identity: PreviewIdentity, cx: &App) -> bool {
    cx.try_global::<DiagramViewers>()
        .is_some_and(|viewers| viewers.map.contains_key(&identity))
}

/// Close every viewer that is not on screen where the editor is drawing:
/// `expected[slot]` is the buffer group `slot` shows (`None` when the group
/// is gone or empty). Run once per frame from the editor's render — it is
/// the one place that knows what each group is showing, and it makes every
/// path that takes a diagram off a group (a file closed in one group, a
/// group emptied or merged away, a tab switched) release the viewer's
/// rasters instead of holding 64 MB nobody can see.
pub(crate) fn sweep(expected: [Option<EntityId>; 2], window: &mut Window, cx: &mut App) {
    // Collect and remove in one borrow; release after it ends.
    let gone: Vec<Viewer> = match with_viewers(cx, |viewers| {
        let identities: Vec<PreviewIdentity> = viewers
            .map
            .iter()
            .filter(|(_, viewer)| {
                !expected
                    .get(viewer.slot)
                    .is_some_and(|buffer| *buffer == Some(viewer.buffer))
            })
            .map(|(identity, _)| *identity)
            .collect();
        identities
            .into_iter()
            .filter_map(|identity| viewers.map.remove(&identity))
            .collect()
    }) {
        Some(gone) => gone,
        None => return,
    };
    if gone.is_empty() {
        return;
    }
    for viewer in gone {
        release(&viewer, window, cx);
    }
    window.refresh();
}

/// `global_mut`, but `None` when the registry has never been installed.
fn with_viewers<R>(cx: &mut App, f: impl FnOnce(&mut DiagramViewers) -> R) -> Option<R> {
    if !cx.has_global::<DiagramViewers>() {
        return None;
    }
    Some(f(cx.global_mut::<DiagramViewers>()))
}

/// Open (or replace) the viewer for `identity`. `seed` is the preview's own
/// image for the block — its raster, CSS size, and bucket — so the first
/// frame is the diagram itself, never a spinner.
pub(crate) fn open(
    identity: PreviewIdentity,
    buffer: EntityId,
    block_id: u64,
    source: &str,
    seed: Option<(Arc<gpui::RenderImage>, viewport::Size, f32, MermaidTheme)>,
    window: &mut Window,
    cx: &mut App,
) {
    install(cx);
    let focus = cx.focus_handle();
    let replaced = cx.global_mut::<DiagramViewers>().map.insert(
        identity,
        Viewer {
            buffer,
            slot: identity.slot(),
            block_id,
            source: source.to_string(),
            view: Viewport {
                scale: 1.,
                offset: (0., 0.),
            },
            natural: seed.as_ref().map(|(_, natural, _, _)| *natural),
            area: None,
            fitted: true,
            raster: seed.map(|(image, natural, bucket, theme)| Raster {
                image,
                natural,
                bucket,
                theme,
                window: Some(window.window_handle()),
                owned: false,
            }),
            retired: None,
            in_flight: None,
            generation: 0,
            error: None,
            drawing_window: Some(window.window_handle()),
            focus,
            drag: None,
            gesture_zoom: None,
            gesture_until: None,
        },
    );
    // A replaced viewer's images go now: nothing will draw them again —
    // and only the ones it owned; the seed belongs to the preview cache.
    if let Some(old) = replaced {
        for gone in [old.raster, old.retired].into_iter().flatten() {
            if gone.owned
                && let Some(handle) = gone.window
            {
                if handle == window.window_handle() {
                    let _ = window.drop_image(gone.image);
                } else {
                    let _ = handle.update(cx, |_, window, _| window.drop_image(gone.image));
                }
            }
        }
    }
    let focus = cx.global::<DiagramViewers>().map[&identity].focus.clone();
    window.focus(&focus, cx);
    window.refresh();
}

/// Close the viewer for `identity`, releasing its images from the atlases
/// they were drawn into.
pub(crate) fn close(identity: PreviewIdentity, window: &mut Window, cx: &mut App) {
    let Some(viewer) = with_viewers(cx, |viewers| viewers.map.remove(&identity)).flatten() else {
        return;
    };
    release(&viewer, window, cx);
    window.refresh();
}

/// Close every viewer opened from `buffer`, wherever it is drawn.
pub(crate) fn close_for_buffer(buffer: EntityId, window: &mut Window, cx: &mut App) {
    if !cx.has_global::<DiagramViewers>() {
        return;
    }
    let viewers = cx.global_mut::<DiagramViewers>();
    let identities: Vec<PreviewIdentity> = viewers
        .map
        .iter()
        .filter(|(_, viewer)| viewer.buffer == buffer)
        .map(|(identity, _)| *identity)
        .collect();
    if identities.is_empty() {
        return;
    }
    let mut gone = Vec::with_capacity(identities.len());
    for identity in identities {
        gone.extend(viewers.map.remove(&identity));
    }
    for viewer in gone {
        release(&viewer, window, cx);
    }
    window.refresh();
}

/// Drop a viewer's images from the atlases that hold them: this window
/// directly, others through their handles, where a closed window fails its
/// update harmlessly — its atlas went with it.
fn release(viewer: &Viewer, window: &mut Window, cx: &mut App) {
    // Each owned image remembers the window that drew it; this window drops
    // it directly and others go through their handles, where a closed
    // window fails harmlessly — its atlas went with it. Unowned images —
    // the seeded preview cache's — are the cache's to manage.
    for raster in [viewer.raster.as_ref(), viewer.retired.as_ref()]
        .into_iter()
        .flatten()
        .filter(|r| r.owned)
    {
        if let Some(handle) = raster.window {
            if handle == window.window_handle() {
                let _ = window.drop_image(raster.image.clone());
            } else {
                let _ = handle.update(cx, |_, window, _| window.drop_image(raster.image.clone()));
            }
        }
    }
}

fn install(cx: &mut App) {
    if !cx.has_global::<DiagramViewers>() {
        cx.set_global(DiagramViewers::default());
    }
}

/// What [`render`] draws one frame from: a copy of the pieces the element
/// tree needs, taken while the global is briefly borrowed.
struct Frame {
    view: Viewport,
    raster: Option<(Arc<gpui::RenderImage>, viewport::Size)>,
    error: Option<String>,
    focus: FocusHandle,
    dragging: bool,
}

/// The overlay for `identity`, or `None` when no viewer is open. Layered
/// over the preview by the editor; covers the text, keeps its own focus,
/// and answers the font-size actions with the diagram's zoom so neither the
/// preview's text zoom nor the terminal's sees them.
pub(crate) fn render(identity: PreviewIdentity, cx: &mut App) -> Option<AnyElement> {
    install(cx);
    if !is_open(identity, cx) {
        return None;
    }
    // The overlay's content is built in prepaint, not here: its area is
    // only known once layout has run, and the fit, the re-fit on resize and
    // the block-gone close all depend on it. State changed from a render
    // closure's prepaint cannot ask for another frame — gpui drops
    // `window.refresh()` and `cx.notify()` mid-draw, and an idle window
    // would keep the stale frame — so the content is made *after* that
    // bookkeeping, from the state it just settled, and the frame that
    // runs it already shows the result.
    Some(
        gpui::canvas(
            move |bounds, window, cx| {
                let mut surface = surface(identity, bounds, window, cx);
                surface.prepaint_as_root(bounds.origin, bounds.size.into(), window, cx);
                surface
            },
            move |bounds, mut surface, window, cx| {
                // Capture-phase zoom: decided against the pointer before any
                // bubble listener can pan or scroll.
                window.on_mouse_event(move |ev: &gpui::ScrollWheelEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture
                        || !bounds.contains(&window.mouse_position())
                    {
                        return;
                    }
                    if decide_zoom(identity, ev, bounds.origin, window, cx) {
                        cx.stop_propagation();
                    }
                });
                surface.paint(window, cx);
            },
        )
        .absolute()
        .inset_0()
        .into_any_element(),
    )
}

/// The overlay for one frame, at its settled size: bookkeeping first (area,
/// fit, retire, theme, block-gone), then the element tree from the state it
/// left. Empty when the viewer closed on the way.
fn surface(
    identity: PreviewIdentity,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let area = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    if !prepare(identity, area, window, cx) {
        return div().into_any_element();
    }
    let Some(frame) = cx
        .global::<DiagramViewers>()
        .map
        .get(&identity)
        .map(|viewer| Frame {
            view: viewer.view,
            raster: viewer.raster.as_ref().map(|r| (r.image.clone(), r.natural)),
            error: viewer.error.clone(),
            focus: viewer.focus.clone(),
            dragging: viewer.drag.is_some(),
        })
    else {
        return div().into_any_element();
    };
    let theme = cx.theme();

    let mut overlay = div()
        .id("diagram-viewer")
        .relative()
        .w(bounds.size.width)
        .h(bounds.size.height)
        .overflow_hidden()
        .occlude()
        .bg(theme.background)
        .track_focus(&frame.focus)
        .when(frame.dragging, |el| el.cursor(CursorStyle::ClosedHand))
        .when(!frame.dragging, |el| el.cursor(CursorStyle::OpenHand))
        .on_action(move |_: &IncreaseFontSize, window, cx| {
            zoom_by(identity, ACTION_ZOOM, None, window, cx);
        })
        .on_action(move |_: &DecreaseFontSize, window, cx| {
            zoom_by(identity, 1. / ACTION_ZOOM, None, window, cx);
        })
        .on_action(move |_: &ResetFontSize, window, cx| fit(identity, window, cx))
        .on_key_down(move |ev, window, cx| {
            if ev.keystroke.key == "escape" {
                close(identity, window, cx);
                cx.stop_propagation();
            }
        })
        // Plain wheel pans — the viewer has no scroll of its own — and the
        // stop keeps the preview underneath from scrolling under the
        // reader's hand.
        .on_scroll_wheel(move |ev, window, cx| {
            let delta = ev.delta.pixel_delta(px(16.));
            pan_by(identity, (delta.x.as_f32(), delta.y.as_f32()), window, cx);
            cx.stop_propagation();
        })
        .on_mouse_down(gpui::MouseButton::Left, move |ev, _window, cx| {
            with_mut(identity, cx, |viewer| viewer.drag = Some(ev.position));
        })
        .on_mouse_move(move |ev, window, cx| {
            let from = with_mut(identity, cx, |viewer| viewer.drag.replace(ev.position)).flatten();
            if let Some(from) = from {
                let delta = (
                    (ev.position.x - from.x).as_f32(),
                    (ev.position.y - from.y).as_f32(),
                );
                pan_by(identity, delta, window, cx);
            }
        })
        .on_mouse_up(gpui::MouseButton::Left, move |_ev, _window, cx| {
            with_mut(identity, cx, |viewer| viewer.drag = None);
        })
        // Releasing outside the overlay ends the drag too, or the viewer
        // would keep panning under a button nobody is holding.
        .on_mouse_up_out(gpui::MouseButton::Left, move |_ev, _window, cx| {
            with_mut(identity, cx, |viewer| viewer.drag = None);
        })
        .on_click(move |ev, window, cx| {
            if ev.click_count() == 2 {
                fit(identity, window, cx);
            }
        });

    // The diagram itself, at the viewport's offset and scale. Without a
    // raster yet (opened before the preview's own image existed) the viewer
    // waits empty; the seed makes this at most one raster away.
    if let Some((image, natural)) = &frame.raster {
        overlay = overlay.child(
            div()
                .id("diagram-viewer-img")
                .absolute()
                .left(px(frame.view.offset.0))
                .top(px(frame.view.offset.1))
                .w(px(natural.0 * frame.view.scale))
                .h(px(natural.1 * frame.view.scale))
                .debug_selector(|| "diagram-viewer-img".to_string())
                .overflow_hidden()
                .child(img(ImageSource::Render(image.clone())).size_full()),
        );
    }
    if let Some(error) = &frame.error {
        let danger = theme.danger;
        overlay = overlay.child(
            div()
                .absolute()
                .top(px(8.))
                .left(px(8.))
                .px(px(8.))
                .py(px(4.))
                .rounded_md()
                .bg(danger.opacity(0.12))
                .text_color(danger)
                .text_size(px(12.))
                .child(format!("{}: {error}", t(L10nKey::MermaidRenderError))),
        );
    }
    overlay
        .child(toolbar(identity, &frame, cx))
        .into_any_element()
}

/// The overlay's once-per-frame look at what its preview witnessed. `None`
/// keeps the viewer (the block was seen, or the preview's view was cached
/// this frame and nothing was witnessed at all); `Some(viewer)` is the
/// viewer, already removed — the fence that made its diagram is gone, and
/// the caller releases its images where a window is in hand.
fn take_witness(identity: PreviewIdentity, cx: &mut App) -> Option<Viewer> {
    let seen = cx
        .global_mut::<DiagramViewers>()
        .witnessed
        .remove(&identity)?;
    let keep = cx
        .global::<DiagramViewers>()
        .map
        .get(&identity)
        .is_some_and(|viewer| seen.contains(&viewer.block_id));
    if keep {
        None
    } else {
        cx.global_mut::<DiagramViewers>().map.remove(&identity)
    }
}

/// The viewer's toolbar: the zoom percentage (against the diagram's natural
/// size), Fit, Close.
fn toolbar(identity: PreviewIdentity, frame: &Frame, cx: &App) -> AnyElement {
    let pct = format!("{}%", (frame.view.scale * 100.).round());
    let theme = cx.theme();
    let chip = |el: gpui::Stateful<gpui::Div>| {
        el.flex()
            .items_center()
            .h(px(24.))
            .px(px(8.))
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(theme.foreground)
            .bg(theme.secondary)
            .hover(move |s| s.bg(theme.secondary_hover))
    };
    v_flex()
        .id("diagram-viewer-toolbar")
        .absolute()
        .top(px(8.))
        .right(px(8.))
        .gap_1()
        .child(chip(div().id("diagram-viewer-zoom")).child(pct))
        .child(
            chip(div().id("diagram-viewer-fit").cursor_pointer())
                .child(t(L10nKey::MermaidViewerFit))
                .on_click(move |_, window, cx| fit(identity, window, cx)),
        )
        .child(
            chip(
                div()
                    .id("diagram-viewer-close")
                    .cursor_pointer()
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(t(L10nKey::MermaidViewerExit))
                            .build(window, cx)
                    }),
            )
            .child(Icon::new(IconName::Close).xsmall())
            .on_click(move |_, window, cx| close(identity, window, cx)),
        )
        .into_any_element()
}

/// The viewer's once-per-frame work, run in the overlay's prepaint with the
/// area it will be drawn in: record the area, re-fit while the viewport is
/// still `fitted`, notice a theme change, retire a replaced image, and run
/// the block-gone check. Returns whether the viewer is still open.
///
/// Everything it changes is read by the element tree built right after it,
/// in the same frame — which is why none of it asks for a repaint.
fn prepare(
    identity: PreviewIdentity,
    area: viewport::Size,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    // Block-gone: the preview's blocks witness themselves as they render;
    // the overlay (a later sibling, so its prepaint runs after the
    // preview's) takes the list once per frame. A list that came back
    // non-empty without our block closes the viewer.
    if let Some(closed) = take_witness(identity, cx) {
        release(&closed, window, cx);
        return false;
    }
    let open = with_mut(identity, cx, |viewer| {
        let resized = viewer.area != Some(area);
        viewer.area = Some(area);
        viewer.drawing_window = Some(window.window_handle());
        // Re-fit on resize while the user has not taken the viewport.
        if resized
            && viewer.fitted
            && let Some(natural) = viewer.natural
        {
            viewer.view = viewport::fit(natural, area);
        }
    })
    .is_some();
    if !open {
        return false;
    }
    // A theme change re-renders at the current bucket.
    let current = crate::ui::theme::mermaid_theme(cx);
    let retheme = with_mut(identity, cx, |viewer| {
        viewer
            .raster
            .as_ref()
            .is_some_and(|r| r.theme != current)
            .then(|| {
                viewer.generation += 1;
                current.clone()
            })
    })
    .flatten();
    if let Some(theme) = retheme {
        schedule(identity, None, Some(theme), window, cx);
    }
    // A replaced image the viewer owned leaves the atlas in the frame that
    // painted its successor — this one.
    let retired = with_mut(identity, cx, |viewer| viewer.retired.take()).flatten();
    if let Some(raster) = retired.filter(|r| r.owned)
        && let Some(drawn_in) = raster.window
    {
        if drawn_in == window.window_handle() {
            let _ = window.drop_image(raster.image);
        } else {
            let _ = drawn_in.update(cx, |_, window, _| window.drop_image(raster.image));
        }
    }
    true
}

/// The wheel decision for one viewer event: pan or zoom, latched per
/// gesture in the terminal's shape. Returns whether the event was consumed
/// as a zoom.
fn decide_zoom(
    identity: PreviewIdentity,
    ev: &gpui::ScrollWheelEvent,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    // The configured modifier zooms, and so does the platform modifier —
    // inside the viewer the setting only *adds* a key; a viewer that ignored
    // Ctrl/Cmd entirely because the setting sits at None would look broken
    // on every fresh install, where None is the default.
    let wants_zoom = zoom_wheel(cx.global::<Config>().mouse_zoom_modifier, &ev.modifiers)
        || (ev.modifiers.number_of_modifiers() == 1 && ev.modifiers.secondary());
    let gesturing = with_mut(identity, cx, |viewer| {
        let now = Instant::now();
        let live = matches!(ev.touch_phase, gpui::TouchPhase::Started)
            || viewer.gesture_until.is_some_and(|until| now < until);
        viewer.gesture_until = live.then(|| now + SCROLL_GESTURE_IDLE);
        live
    })
    .unwrap_or(false);
    if !gesturing || matches!(ev.touch_phase, gpui::TouchPhase::Started) {
        with_mut(identity, cx, |viewer| viewer.gesture_zoom = None);
    }
    let zoom = with_mut(identity, cx, |viewer| {
        if gesturing {
            *viewer.gesture_zoom.get_or_insert(wants_zoom)
        } else {
            wants_zoom
        }
    })
    .unwrap_or(false);
    if !zoom {
        return false;
    }
    let dy = match ev.delta {
        gpui::ScrollDelta::Lines(p) => p.y * 16.,
        gpui::ScrollDelta::Pixels(p) => p.y.as_f32(),
    };
    // The viewport's offsets are in the overlay's own coordinates, and the
    // overlay sits wherever the editor group does — not at the window's
    // corner — so the pointer is taken relative to it.
    let at = Some((
        (ev.position.x - origin.x).as_f32(),
        (ev.position.y - origin.y).as_f32(),
    ));
    zoom_by(identity, viewport::wheel_factor(dy), at, window, cx);
    true
}

/// Zoom by `factor`, about `at` (in the overlay's coordinates) or the area's
/// centre.
fn zoom_by(
    identity: PreviewIdentity,
    factor: f32,
    at: Option<(f32, f32)>,
    window: &mut Window,
    cx: &mut App,
) {
    let scale_changed = with_mut(identity, cx, |viewer| {
        let Some(area) = viewer.area else {
            return false;
        };
        let at = at.unwrap_or((area.0 / 2., area.1 / 2.));
        let next = viewport::zoom_about(viewer.view, factor, at);
        let changed = next != viewer.view;
        viewer.view = next;
        viewer.fitted = false;
        changed
    })
    .unwrap_or(false);
    if scale_changed {
        schedule(identity, None, None, window, cx);
        window.refresh();
    }
}

/// Pan by `delta`, keeping the diagram grabbable.
fn pan_by(identity: PreviewIdentity, delta: (f32, f32), window: &mut Window, cx: &mut App) {
    let moved = with_mut(identity, cx, |viewer| {
        let (Some(natural), Some(area)) = (viewer.natural, viewer.area) else {
            return false;
        };
        let next = viewport::pan(viewer.view, delta, natural, area);
        let moved = next != viewer.view;
        viewer.view = next;
        viewer.fitted = false;
        moved
    })
    .unwrap_or(false);
    if moved {
        window.refresh();
    }
}

/// Fit the diagram to the area again.
fn fit(identity: PreviewIdentity, window: &mut Window, cx: &mut App) {
    with_mut(identity, cx, |viewer| {
        if let (Some(natural), Some(area)) = (viewer.natural, viewer.area) {
            viewer.view = viewport::fit(natural, area);
            viewer.fitted = true;
        }
    });
    schedule(identity, None, None, window, cx);
    window.refresh();
}

/// Request the raster for the viewer's zoom, unless the one on screen (or
/// on its way) already covers it. `theme` overrides the current palette for
/// one re-render. The generation was already bumped by whoever decided a
/// change is due; the render answers to it.
fn schedule(
    identity: PreviewIdentity,
    bucket: Option<f32>,
    theme: Option<MermaidTheme>,
    window: &mut Window,
    cx: &mut App,
) {
    let bucket = bucket.unwrap_or_else(|| {
        viewport::raster_bucket(view_scale(identity, cx), window.scale_factor())
    });
    let theme = theme.unwrap_or_else(|| crate::ui::theme::mermaid_theme(cx));
    let wanted = with_mut(identity, cx, |viewer| {
        if viewer
            .raster
            .as_ref()
            .is_some_and(|r| r.bucket == bucket && r.theme == theme)
        {
            return None;
        }
        if viewer
            .in_flight
            .as_ref()
            .is_some_and(|f| f.bucket == bucket && f.theme == theme)
        {
            return None;
        }
        Some((viewer.source.clone(), viewer.generation))
    })
    .flatten();
    let Some((source, generation)) = wanted else {
        return;
    };
    let for_render = theme.clone();
    let for_complete = theme.clone();
    let for_flight = theme.clone();
    let task = cx.spawn(async move |cx| {
        // Let the zoom settle first: a flick crosses several buckets on its
        // way and only the one it stops at is worth rendering.
        cx.background_executor().timer(SETTLE).await;
        let rendered = cx
            .background_spawn(async move { tty59_mermaid::render(&source, &for_render, bucket) })
            .await;
        cx.update(move |cx| {
            complete(identity, bucket, for_complete, generation, rendered, cx);
        });
    });
    with_mut(identity, cx, |viewer| {
        viewer.in_flight = Some(InFlight {
            bucket,
            theme: for_flight,
            _task: task,
        });
    });
}

/// A requested raster landed: swap it in when the viewer still wants it,
/// retire what it replaces, and refresh. A failure with nothing on screen
/// closes the viewer; with something, it becomes the error line over it.
fn complete(
    identity: PreviewIdentity,
    bucket: f32,
    theme: MermaidTheme,
    generation: u64,
    rendered: Result<tty59_mermaid::Raster, tty59_mermaid::MermaidError>,
    cx: &mut App,
) {
    enum Outcome {
        Applied,
        Close,
        Stale,
    }
    let outcome = with_mut(identity, cx, |viewer| {
        if viewer.generation != generation {
            return Outcome::Stale;
        }
        viewer.in_flight = None;
        let failure = rendered
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        match rendered
            .ok()
            .and_then(raster_to_image)
            .map(|(image, logical)| (image, (logical.width, logical.height)))
        {
            Some((image, natural)) => {
                let retired = viewer.raster.take();
                let window = viewer
                    .drawing_window
                    .or(retired.as_ref().and_then(|r| r.window));
                viewer.raster = Some(Raster {
                    image,
                    natural,
                    bucket,
                    theme,
                    window,
                    owned: true,
                });
                if viewer.natural.is_none() {
                    viewer.natural = Some(natural);
                    if viewer.fitted
                        && let Some(area) = viewer.area
                    {
                        viewer.view = viewport::fit(natural, area);
                    }
                }
                viewer.retired = retired;
                viewer.error = None;
                Outcome::Applied
            }
            None => {
                if viewer.raster.is_some() {
                    viewer.error = Some(failure);
                    Outcome::Applied
                } else {
                    Outcome::Close
                }
            }
        }
    });
    match outcome {
        Some(Outcome::Applied) => cx.refresh_windows(),
        Some(Outcome::Stale) | None => {}
        Some(Outcome::Close) => {
            // Closing releases images, which needs a window; any live one
            // will do, and the refresh lands with it.
            if let Some(handle) = cx.windows().first().copied() {
                let _ = handle.update(cx, |_, window, cx| {
                    close(identity, window, cx);
                });
            }
        }
    }
}

/// The viewer's current zoom, for bucket math outside a `with_mut` borrow.
fn view_scale(identity: PreviewIdentity, cx: &App) -> f32 {
    cx.try_global::<DiagramViewers>()
        .and_then(|viewers| viewers.map.get(&identity))
        .map(|viewer| viewer.view.scale)
        .unwrap_or(1.)
}

/// Run `f` on the viewer for `identity`, if one is open.
fn with_mut<R>(
    identity: PreviewIdentity,
    cx: &mut App,
    f: impl FnOnce(&mut Viewer) -> R,
) -> Option<R> {
    with_viewers(cx, |viewers| viewers.map.get_mut(&identity).map(f)).flatten()
}

/// A seed raster for tests, here and in the editor's: 100×50 CSS px,
/// rendered at 2× device scale.
#[cfg(test)]
pub(crate) fn test_seed(cx: &App) -> (Arc<gpui::RenderImage>, viewport::Size, f32, MermaidTheme) {
    let (image, logical) = raster_to_image(tty59_mermaid::Raster {
        width: 200,
        height: 100,
        scale: 2.0,
        bgra: vec![0x40; 200 * 100 * 4],
    })
    .expect("a well-formed raster");
    (
        image,
        (logical.width, logical.height),
        2.0,
        crate::ui::theme::mermaid_theme(cx),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// A window root that draws the viewer overlay for one identity, the
    /// way the editor's preview arm does.
    struct Host {
        identity: PreviewIdentity,
    }

    impl gpui::Render for Host {
        fn render(
            &mut self,
            _window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            // The way the editor mounts it: the overlay is absolutely
            // positioned over a full-size relative parent — the window root
            // itself has no positioned ancestor to pin to.
            render(self.identity, cx)
                .map(|overlay| {
                    div()
                        .relative()
                        .size_full()
                        .child(overlay)
                        .into_any_element()
                })
                .unwrap_or_else(|| div().into_any_element())
        }
    }

    fn identity(buffer: u64, slot: usize) -> PreviewIdentity {
        PreviewIdentity::from_buffer(EntityId::from(buffer), slot)
    }

    fn seed(cx: &App) -> (Arc<gpui::RenderImage>, viewport::Size, f32, MermaidTheme) {
        test_seed(cx)
    }

    fn settle(vcx: &mut gpui::VisualTestContext) {
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        vcx.run_until_parked();
    }

    /// The viewer's whole session: open registers it (and focuses it), the
    /// preview saying the block is gone closes it, Escape closes it, and
    /// closing the buffer clears every entry of its.
    #[gpui::test]
    fn the_viewer_lifecycle(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let the_identity = identity(7, 0);
        let (_, mut vcx) = cx.add_window_view(|window, cx| {
            let host = cx.new(|_| Host {
                identity: the_identity,
            });
            gpui_component::Root::new(host, window, cx)
        });

        // Open: registered, focused.
        vcx.update(|window, cx| {
            open(
                the_identity,
                EntityId::from(7),
                42,
                "graph TD\nA-->B",
                Some(seed(cx)),
                window,
                cx,
            );
        });
        assert!(vcx.update(|_, cx| is_open(the_identity, cx)));
        settle(&mut vcx);
        vcx.update(|window, cx| {
            assert!(
                cx.global::<DiagramViewers>().map[&the_identity]
                    .focus
                    .is_focused(window),
                "the viewer takes the focus"
            );
        });

        // A preview pass that witnessed other blocks closes it.
        vcx.update(|_window, cx| {
            witness(the_identity, 99, "graph TD\nA-->B", cx);
            assert!(
                take_witness(the_identity, cx).is_some(),
                "a block-less pass closes the viewer"
            );
        });
        assert!(!vcx.update(|_, cx| is_open(the_identity, cx)));

        // Escape: reopen, draw so the overlay and its key handler are up,
        // then Esc.
        vcx.update(|window, cx| {
            open(
                the_identity,
                EntityId::from(7),
                42,
                "graph TD\nA-->B",
                Some(seed(cx)),
                window,
                cx,
            );
        });
        settle(&mut vcx);
        vcx.simulate_keystrokes("escape");
        settle(&mut vcx);
        assert!(
            !vcx.update(|_, cx| is_open(the_identity, cx)),
            "Escape closes the viewer"
        );

        // Closing the buffer clears every entry of its — both groups.
        let other_slot = identity(7, 1);
        vcx.update(|window, cx| {
            open(
                the_identity,
                EntityId::from(7),
                42,
                "graph TD\nA-->B",
                Some(seed(cx)),
                window,
                cx,
            );
            open(
                other_slot,
                EntityId::from(7),
                43,
                "graph TD\nA-->C",
                Some(seed(cx)),
                window,
                cx,
            );
        });
        vcx.update(|window, cx| close_for_buffer(EntityId::from(7), window, cx));
        assert!(!vcx.update(|_, cx| is_open(the_identity, cx)));
        assert!(!vcx.update(|_, cx| is_open(other_slot, cx)));
    }

    /// A frame runs only when something asked for one: the test harness
    /// draws a dirty window after every update and not otherwise, which is
    /// what the platform does too. So the tests below never draw by hand —
    /// what `debug_bounds` reports is what the frames the actions asked for
    /// painted, and a mutation that wanted a repaint it never got shows up
    /// as the stale picture it left.
    fn painted(vcx: &mut gpui::VisualTestContext) -> Bounds<Pixels> {
        vcx.debug_bounds("diagram-viewer-img")
            .expect("the diagram is painted")
    }

    fn close_to(got: f32, want: f32, what: &str) {
        assert!((got - want).abs() < 0.6, "{what}: got {got}, want {want}");
    }

    /// The shared rig for the interaction tests: a 400×600 window with a
    /// viewer opened over it on a 100×50 diagram, `mouse_zoom_modifier` at
    /// its default (None) — the case the wheel tests exist for.
    fn interactive_viewer(cx: &mut TestAppContext) -> (PreviewIdentity, gpui::VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut config = Config::default();
            config.mouse_zoom_modifier = crate::core::config::MouseZoomModifier::None;
            cx.set_global(config);
        });
        let the_identity = identity(13, 0);
        let window = cx.open_window(gpui::size(px(400.), px(600.)), |_window, _cx| Host {
            identity: the_identity,
        });
        let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            open(
                the_identity,
                EntityId::from(13),
                42,
                "graph TD\nA-->B",
                Some(seed(cx)),
                window,
                cx,
            );
        });
        vcx.run_until_parked();
        (the_identity, vcx)
    }

    fn view_of(identity: PreviewIdentity, cx: &App) -> Viewport {
        cx.global::<DiagramViewers>().map[&identity].view
    }

    /// Opening paints the fit in the first frame: in a 400×600 window a
    /// 100×50 diagram fits by width at 352 % with the margin on the sides,
    /// centred vertically. The fit needs the area, which only layout knows,
    /// so a viewer that mutated its view in prepaint — where gpui drops the
    /// repaint request — painted the diagram at 100 % in the corner and
    /// stayed there on an idle window.
    #[gpui::test]
    fn the_first_frame_paints_the_fit(cx: &mut TestAppContext) {
        let (the_identity, mut vcx) = interactive_viewer(cx);
        let view = vcx.update(|_, cx| view_of(the_identity, cx));
        close_to(view.scale, 3.52, "fitted scale");
        let img = painted(&mut vcx);
        // (400 − 2·24) / 100 = 3.52; centred: (400−352)/2 = 24,
        // (600 − 50·3.52)/2 = 212.
        close_to(img.size.width.as_f32(), 352., "painted width");
        close_to(img.size.height.as_f32(), 176., "painted height");
        close_to(img.origin.x.as_f32(), 24., "painted x");
        close_to(img.origin.y.as_f32(), 212., "painted y");
    }

    /// Resizing the window re-fits a viewer nobody has zoomed, and the same
    /// frame that learns the new size paints the new fit.
    #[gpui::test]
    fn a_resize_refits_in_the_frame_that_sees_it(cx: &mut TestAppContext) {
        let (_, mut vcx) = interactive_viewer(cx);
        vcx.simulate_resize(gpui::size(px(500.), px(700.)));
        vcx.run_until_parked();
        let img = painted(&mut vcx);
        // (500 − 48) / 100 = 4.52, centred: 24, (700 − 226)/2 = 237.
        close_to(img.size.width.as_f32(), 452., "painted width");
        close_to(img.origin.x.as_f32(), 24., "painted x");
        close_to(img.origin.y.as_f32(), 237., "painted y");
    }

    /// A left drag pans — in both axes, since a fitted diagram is not locked
    /// to the centre — and the frame the drag asked for paints it.
    #[gpui::test]
    fn a_drag_pans_the_viewer_and_paints(cx: &mut TestAppContext) {
        let (_, mut vcx) = interactive_viewer(cx);
        let before = painted(&mut vcx);

        let start = gpui::point(px(200.), px(300.));
        let end = gpui::point(px(160.), px(280.));
        vcx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
        vcx.simulate_mouse_move(end, Some(gpui::MouseButton::Left), gpui::Modifiers::none());
        vcx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::none());
        vcx.run_until_parked();

        let after = painted(&mut vcx);
        close_to(
            after.origin.x.as_f32(),
            before.origin.x.as_f32() - 40.,
            "painted x",
        );
        close_to(
            after.origin.y.as_f32(),
            before.origin.y.as_f32() - 20.,
            "painted y",
        );
    }

    /// A plain wheel pans, and the frame it asked for paints it.
    #[gpui::test]
    fn a_plain_wheel_pans_the_viewer_and_paints(cx: &mut TestAppContext) {
        let (_, mut vcx) = interactive_viewer(cx);
        let before = painted(&mut vcx);
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(px(200.), px(300.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-30.))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        let after = painted(&mut vcx);
        close_to(
            after.origin.y.as_f32(),
            before.origin.y.as_f32() - 30.,
            "painted y",
        );
        close_to(
            after.size.width.as_f32(),
            before.size.width.as_f32(),
            "scale",
        );
    }

    /// Ctrl+wheel zooms about the pointer with the default
    /// `mouse_zoom_modifier: None` — the platform modifier always zooms
    /// inside the viewer — and what is painted afterwards is the zoomed
    /// diagram with the point under the pointer still under it. Wheel up
    /// (gpui's dy > 0, X button 4) zooms in, as it does over the editor and
    /// the terminal; wheel down zooms out.
    #[gpui::test]
    fn ctrl_wheel_zooms_the_viewer_about_the_pointer(cx: &mut TestAppContext) {
        let (_, mut vcx) = interactive_viewer(cx);
        let before = painted(&mut vcx);

        let at = gpui::point(px(200.), px(300.));
        let ctrl_wheel = |dy: f32| gpui::ScrollWheelEvent {
            position: at,
            delta: gpui::ScrollDelta::Lines(gpui::point(0., dy)),
            modifiers: gpui::Modifiers::control(),
            touch_phase: gpui::TouchPhase::Moved,
        };
        vcx.simulate_event(ctrl_wheel(3.));
        vcx.run_until_parked();

        let after = painted(&mut vcx);
        assert!(
            after.size.width.as_f32() > before.size.width.as_f32() * 1.03,
            "wheel up zooms in: painted width {} -> {}",
            before.size.width.as_f32(),
            after.size.width.as_f32()
        );
        // About the pointer: the diagram point under (200, 300) stays there.
        let ratio = after.size.width.as_f32() / before.size.width.as_f32();
        close_to(
            after.origin.x.as_f32(),
            200. - (200. - before.origin.x.as_f32()) * ratio,
            "anchor x",
        );
        close_to(
            after.origin.y.as_f32(),
            300. - (300. - before.origin.y.as_f32()) * ratio,
            "anchor y",
        );

        // And the other way round: wheel down zooms back out.
        vcx.simulate_event(ctrl_wheel(-3.));
        vcx.run_until_parked();
        let back = painted(&mut vcx);
        assert!(
            back.size.width.as_f32() < after.size.width.as_f32() / 1.03,
            "wheel down zooms out: painted width {} -> {}",
            after.size.width.as_f32(),
            back.size.width.as_f32()
        );
    }

    /// The viewer closing because its block is gone takes the overlay off
    /// the very frame that noticed: nothing is left painted over the text.
    #[gpui::test]
    fn a_vanished_block_clears_the_overlay_in_the_same_frame(cx: &mut TestAppContext) {
        let (the_identity, mut vcx) = interactive_viewer(cx);
        vcx.update(|window, cx| {
            // What a preview pass that drew other blocks leaves behind, and
            // the one frame that asks for a draw.
            witness(the_identity, 99, "graph TD\nA-->B", cx);
            window.refresh();
        });
        vcx.run_until_parked();
        assert!(!vcx.update(|_, cx| is_open(the_identity, cx)));
        assert!(
            vcx.debug_bounds("diagram-viewer-img").is_none(),
            "no diagram left painted"
        );
    }

    /// The pass that keeps the viewer alive: the block was witnessed, so
    /// nothing closes — and a cached-preview frame (nothing witnessed at
    /// all) is no verdict either way.
    #[gpui::test]
    fn a_witnessed_block_keeps_the_viewer(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let the_identity = identity(9, 0);
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let host = cx.new(|_| Host {
                identity: the_identity,
            });
            gpui_component::Root::new(host, window, cx)
        });
        vcx.update(|window, cx| {
            open(
                the_identity,
                EntityId::from(9),
                5,
                "graph TD\nA-->B",
                Some(seed(cx)),
                window,
                cx,
            );
        });
        vcx.update(|_window, cx| {
            witness(the_identity, 5, "graph TD\nA-->B", cx);
            assert!(
                take_witness(the_identity, cx).is_none(),
                "the block was witnessed: keep"
            );
            assert!(
                take_witness(the_identity, cx).is_none(),
                "no witnesses means no verdict"
            );
        });
        assert!(vcx.update(|_, cx| is_open(the_identity, cx)));
    }
}
