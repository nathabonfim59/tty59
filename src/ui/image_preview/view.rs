//! The picture, shown: a document's decoded content and the views onto it.
//!
//! [`ImageData`] is the document, and it is an entity so that its id can be a
//! `BufferId` like a text buffer's: the strip, the split groups, close and
//! restore all speak in those ids already. It holds what was decoded and
//! nothing about where it is shown.
//!
//! [`ImageView`] is one place it is shown — one editor group of one tab. The
//! viewport, the size it was laid out in and the focus belong to the view, so
//! the same picture open in two groups of different sizes fits and zooms
//! independently; only the decoded pixels are shared.
//!
//! The viewport maths is the diagram viewer's ([`viewport`]); the handlers
//! have the same shape: ctrl/cmd + wheel zooms about the pointer, a plain wheel
//! and a middle drag pan, a double click toggles fit and 100 %, and the
//! font-size actions zoom the picture instead of the terminal.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, Entity, FocusHandle, ImageSource, MouseButton,
    Pixels, Point, Render, RenderImage, SharedString, Window, canvas, div, fill, img, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::decode::{Animation, Decoded, Format, ImageError, MAX_IMAGE_BYTES, MAX_PIXELS};
use super::decode::{MAX_RGBA_BYTES, Unsupported};
use crate::core::actions::{DecreaseFontSize, IncreaseFontSize, ResetFontSize};
use crate::core::config::Config;
use crate::ui::i18n::{L10nKey, t, t_fmt};
use crate::ui::markdown_mermaid::viewport::{self, Limits, Viewport};
use crate::ui::zoom::{SCROLL_GESTURE_IDLE, zoom_wheel};

/// The zoom the font-size actions send: coarse on purpose, they are the
/// keyboard and not the gesture.
const ACTION_ZOOM: f32 = 1.25;
/// The size of one square of the transparency checkerboard, in logical px.
const CHECKER: f32 = 12.;

const MIB: u64 = 1024 * 1024;

/// A picture that was decoded, or why it was not.
pub(crate) enum Content {
    Ready(Shown),
    Failed(Failure),
}

/// A decoded picture, ready to draw.
pub(crate) struct Shown {
    /// The pixels, shared by every view of the document. Dropped from the
    /// window's atlas when the document closes (see `release`).
    pub(crate) image: Arc<RenderImage>,
    pub(crate) format: Format,
    /// What 100 % is, in logical px.
    pub(crate) natural: (f32, f32),
    /// The pixels the image really has: differs from `natural` for an SVG and
    /// for a reduced picture.
    pub(crate) pixels: (u32, u32),
    pub(crate) bytes: u64,
    pub(crate) animation: Animation,
    pub(crate) reduced: bool,
    /// Whether any pixel is not opaque, so a checkerboard shows behind it.
    pub(crate) has_alpha: bool,
}

/// A picture that cannot be shown, and what to say about it.
pub(crate) struct Failure {
    pub(crate) error: ImageError,
    /// The file's size, when it was known, for the words about a limit.
    pub(crate) bytes: Option<u64>,
}

/// One open picture, whatever shows it.
pub(crate) struct ImageData {
    pub(crate) path: PathBuf,
    pub(crate) content: Content,
}

impl ImageData {
    pub(crate) fn ready(path: PathBuf, decoded: Decoded, has_alpha: bool) -> Self {
        let reduced = decoded.reduced();
        let Decoded {
            format,
            natural,
            pixels,
            frames,
            animation,
            bytes,
            ..
        } = decoded;
        Self {
            path,
            content: Content::Ready(Shown {
                image: Arc::new(RenderImage::new(frames)),
                format,
                natural,
                pixels,
                bytes,
                animation,
                reduced,
                has_alpha,
            }),
        }
    }

    pub(crate) fn failed(path: PathBuf, error: ImageError, bytes: Option<u64>) -> Self {
        Self {
            path,
            content: Content::Failed(Failure { error, bytes }),
        }
    }

    /// The file's name, which is what the strip and the messages call it.
    pub(crate) fn name(&self) -> SharedString {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
            .into()
    }

    /// The pixels to drop from the atlas when the document goes.
    pub(crate) fn image(&self) -> Option<Arc<RenderImage>> {
        match &self.content {
            Content::Ready(shown) => Some(shown.image.clone()),
            Content::Failed(_) => None,
        }
    }
}

/// Whether any pixel of a BGRA frame is not opaque.
pub(crate) fn has_alpha(bgra: &[u8]) -> bool {
    bgra.chunks_exact(4).any(|px| px[3] != 255)
}

/// A count with its thousands set off by commas: `16,777,216`.
pub(crate) fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `812 B`, `1.2 KiB`, `3.4 MiB`.
pub(crate) fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KIB * KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{:.1} MiB", b / (KIB * KIB))
    }
}

/// A size in the info line: whole numbers stay whole.
fn dimension(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

/// What to say about a picture that is not shown, in the user's language.
pub(crate) fn failure_message(name: &str, path: &Path, failure: &Failure) -> String {
    let _ = path;
    let mib = |bytes: u64| bytes.div_ceil(MIB).to_string();
    match &failure.error {
        // A size is only claimed when it was measured: a file that grew past
        // the limit between the size check and the read has no size to name.
        ImageError::TooLarge { .. } if failure.bytes.is_none() => t_fmt(
            L10nKey::ImagePreviewTooLargeUnknown,
            &[
                ("name", name),
                ("limit", &(MAX_IMAGE_BYTES / MIB).to_string()),
            ],
        ),
        ImageError::TooLarge { bytes } => t_fmt(
            L10nKey::ImagePreviewTooLarge,
            &[
                ("name", name),
                ("size", &mib(*bytes)),
                ("limit", &(MAX_IMAGE_BYTES / MIB).to_string()),
            ],
        ),
        ImageError::TooManyPixels { width, height } => t_fmt(
            L10nKey::ImagePreviewTooManyPixels,
            &[
                ("name", name),
                ("width", &width.to_string()),
                ("height", &height.to_string()),
                ("limit", &grouped(MAX_PIXELS)),
            ],
        ),
        ImageError::OverBudget => t_fmt(
            L10nKey::ImagePreviewOverBudget,
            &[
                ("name", name),
                ("limit", &(MAX_RGBA_BYTES / MIB).to_string()),
            ],
        ),
        ImageError::Corrupt => t_fmt(L10nKey::ImagePreviewCorrupt, &[("name", name)]),
        ImageError::Unsupported(Unsupported::Format(format)) => t_fmt(
            L10nKey::ImagePreviewUnsupportedFormat,
            &[("name", name), ("format", format)],
        ),
        ImageError::Unsupported(Unsupported::Feature) => {
            t_fmt(L10nKey::ImagePreviewUnsupportedFeature, &[("name", name)])
        }
        ImageError::Unsupported(Unsupported::SvgImages) => {
            t_fmt(L10nKey::ImagePreviewSvgImages, &[("name", name)])
        }
        ImageError::NotAnImage => t_fmt(L10nKey::ImagePreviewNotAnImage, &[("name", name)]),
    }
}

/// One place a document is shown.
pub(crate) struct ImageView {
    data: Entity<ImageData>,
    view: Viewport,
    /// The size the view was last laid out in, to re-fit on resize.
    area: Option<viewport::Size>,
    /// Re-fit on resize while the reader has not zoomed or panned.
    fitted: bool,
    pub(crate) focus: FocusHandle,
    drag: Option<Point<Pixels>>,
    /// The wheel latch, in the terminal's shape: a gesture answers "pan or
    /// zoom?" once and keeps the answer until the stream dies.
    gesture_zoom: Option<bool>,
    gesture_until: Option<Instant>,
    /// Which editor group this view is drawn in, to tell two groups' painted
    /// parts apart.
    slot: usize,
}

/// A painted part's selector: the plain name in the first group, suffixed in
/// the second, so two groups showing pictures can each be found.
fn selector(base: &str, slot: usize) -> String {
    match slot {
        0 => base.to_owned(),
        n => format!("{base}-{n}"),
    }
}

impl ImageView {
    pub(crate) fn new(data: Entity<ImageData>, slot: usize, cx: &mut Context<Self>) -> Self {
        Self {
            slot,
            data,
            view: Viewport {
                scale: 1.,
                offset: (0., 0.),
            },
            area: None,
            fitted: true,
            focus: cx.focus_handle(),
            drag: None,
            gesture_zoom: None,
            gesture_until: None,
        }
    }

    /// What 100 % is, and whether the picture is a vector: `None` while the
    /// document holds an error.
    fn geometry(&self, cx: &App) -> Option<(viewport::Size, bool)> {
        match &self.data.read(cx).content {
            Content::Ready(shown) => Some((shown.natural, shown.format == Format::Svg)),
            Content::Failed(_) => None,
        }
    }

    /// The zoom range for `natural` in `area`: the diagram's ceiling, and a
    /// floor that is the fit scale when that is lower than the diagram's, so
    /// a panorama can always be brought back to fit.
    fn limits(natural: viewport::Size, area: viewport::Size) -> Limits {
        // The fit scale itself, however small: a picture at the pixel limit
        // can be 1 px thin and 16 777 216 px long, and fitting it takes a
        // scale near 3e-5. Only an area with no room at all (fit 0) or a
        // size that is not a number falls back to the diagram's floor.
        let fit = viewport::fit_scale(natural, area);
        let floor = if fit.is_finite() && fit > 0. {
            fit.min(viewport::MIN_SCALE)
        } else {
            viewport::MIN_SCALE
        };
        Limits {
            min: floor,
            max: viewport::MAX_SCALE,
        }
    }

    /// Fit: the whole picture inside the area, aspect kept. A raster is never
    /// enlarged past its own pixels by fitting (that would only blur it);
    /// a vector picture may be, as a diagram is.
    fn fit_view(natural: viewport::Size, vector: bool, area: viewport::Size) -> Viewport {
        let mut limits = Self::limits(natural, area);
        if !vector {
            limits.max = 1.;
        }
        viewport::fit_in(natural, area, limits)
    }

    /// Once per frame, with the area the view will be drawn in.
    fn prepare(&mut self, area: viewport::Size, cx: &App) {
        let resized = self.area != Some(area);
        self.area = Some(area);
        if resized
            && self.fitted
            && let Some((natural, vector)) = self.geometry(cx)
        {
            self.view = Self::fit_view(natural, vector, area);
        }
    }

    fn zoom_by(&mut self, factor: f32, at: Option<(f32, f32)>, cx: &App) -> bool {
        let (Some((natural, _)), Some(area)) = (self.geometry(cx), self.area) else {
            return false;
        };
        let at = at.unwrap_or((area.0 / 2., area.1 / 2.));
        let next = viewport::zoom_about_in(self.view, factor, at, Self::limits(natural, area));
        let changed = next != self.view;
        self.view = next;
        self.fitted = false;
        changed
    }

    fn pan_by(&mut self, delta: (f32, f32), cx: &App) -> bool {
        let (Some((natural, _)), Some(area)) = (self.geometry(cx), self.area) else {
            return false;
        };
        let next = viewport::pan(self.view, delta, natural, area);
        let moved = next != self.view;
        self.view = next;
        self.fitted = false;
        moved
    }

    fn fit(&mut self, cx: &App) {
        if let (Some((natural, vector)), Some(area)) = (self.geometry(cx), self.area) {
            self.view = Self::fit_view(natural, vector, area);
            self.fitted = true;
        }
    }

    /// 100 %: one logical px per image px, centred.
    fn actual_size(&mut self, cx: &App) {
        if let (Some((natural, _)), Some(area)) = (self.geometry(cx), self.area) {
            self.view = Viewport {
                scale: 1.,
                offset: ((area.0 - natural.0) / 2., (area.1 - natural.1) / 2.),
            };
            self.fitted = false;
        }
    }

    /// A double click toggles between fit and 100 %.
    fn toggle(&mut self, cx: &App) {
        if self.fitted {
            self.actual_size(cx);
        } else {
            self.fit(cx);
        }
    }

    /// The wheel decision for one event: pan or zoom, latched per gesture in
    /// the terminal's shape. Returns whether it was taken as a zoom.
    fn wheel(
        &mut self,
        ev: &gpui::ScrollWheelEvent,
        origin: Point<Pixels>,
        cx: &App,
    ) -> Option<bool> {
        // The configured modifier zooms, and so does the platform one: inside
        // the viewer the setting only *adds* a key, or it would look broken on
        // every fresh install, where the setting is None.
        let wants_zoom = zoom_wheel(cx.global::<Config>().mouse_zoom_modifier, &ev.modifiers)
            || (ev.modifiers.number_of_modifiers() == 1 && ev.modifiers.secondary());
        let now = Instant::now();
        let started = matches!(ev.touch_phase, gpui::TouchPhase::Started);
        let live = started || self.gesture_until.is_some_and(|until| now < until);
        self.gesture_until = live.then(|| now + SCROLL_GESTURE_IDLE);
        if !live || started {
            self.gesture_zoom = None;
        }
        let zoom = if live {
            *self.gesture_zoom.get_or_insert(wants_zoom)
        } else {
            wants_zoom
        };
        if !zoom {
            return None;
        }
        let dy = match ev.delta {
            gpui::ScrollDelta::Lines(p) => p.y * 16.,
            gpui::ScrollDelta::Pixels(p) => p.y.as_f32(),
        };
        // Offsets are in the view's own coordinates, and the view sits
        // wherever its group does, so the pointer is taken relative to it.
        let at = (
            (ev.position.x - origin.x).as_f32(),
            (ev.position.y - origin.y).as_f32(),
        );
        Some(self.zoom_by(viewport::wheel_factor(dy), Some(at), cx))
    }

    /// The picture, its controls and its info for one frame, at the settled
    /// area. Built from the state `prepare` just left.
    fn surface(
        view: &Entity<Self>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let area = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
        let view_bounds = bounds;
        view.update(cx, |this, cx| this.prepare(area, cx));
        let this = view.read(cx);
        let data = this.data.read(cx);
        let Content::Ready(shown) = &data.content else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (view_state, focus, dragging) = (this.view, this.focus.clone(), this.drag.is_some());
        let slot = this.slot;
        let (image, natural, has_alpha) = (shown.image.clone(), shown.natural, shown.has_alpha);
        let info = Self::info_lines(&data.name(), shown);
        let (background, muted) = (theme.background, theme.muted);
        let (foreground, chip_bg, chip_hover) =
            (theme.foreground, theme.secondary, theme.secondary_hover);

        let handle = view.clone();
        let mut body = div()
            .id("image-preview-surface")
            .relative()
            .w(bounds.size.width)
            .h(bounds.size.height)
            .overflow_hidden()
            .bg(background)
            .key_context(crate::ui::code_editor::DOCUMENT_KEY_CONTEXT)
            .track_focus(&focus)
            .cursor(if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::Arrow
            })
            .on_action({
                let view = handle.clone();
                move |_: &IncreaseFontSize, window, cx| {
                    view.update(cx, |v, cx| v.zoom_by(ACTION_ZOOM, None, cx));
                    window.refresh();
                }
            })
            .on_action({
                let view = handle.clone();
                move |_: &DecreaseFontSize, window, cx| {
                    view.update(cx, |v, cx| v.zoom_by(1. / ACTION_ZOOM, None, cx));
                    window.refresh();
                }
            })
            .on_action({
                let view = handle.clone();
                move |_: &ResetFontSize, window, cx| {
                    view.update(cx, |v, cx| v.fit(cx));
                    window.refresh();
                }
            })
            // A plain wheel pans: the viewer has no scroll of its own.
            .on_scroll_wheel({
                let view = handle.clone();
                move |ev, window, cx| {
                    let delta = ev.delta.pixel_delta(px(16.));
                    let delta = (delta.x.as_f32(), delta.y.as_f32());
                    if view.update(cx, |v, cx| v.pan_by(delta, cx)) {
                        window.refresh();
                    }
                    cx.stop_propagation();
                }
            })
            // Only the middle button drags: hovering or pressing the primary
            // button must never move the picture.
            .on_mouse_down(MouseButton::Middle, {
                let view = handle.clone();
                move |ev, window, cx| {
                    view.update(cx, |v, _| v.drag = Some(ev.position));
                    window.refresh();
                }
            })
            .on_mouse_move({
                let view = handle.clone();
                move |ev, window, cx| {
                    if ev.pressed_button != Some(MouseButton::Middle) {
                        // A move without the button ends a drag whose release
                        // was missed, so the cursor goes back to the arrow.
                        if view.update(cx, |v, _| v.drag.take().is_some()) {
                            window.refresh();
                        }
                        return;
                    }
                    let from = view.update(cx, |v, _| {
                        v.drag
                            .as_mut()
                            .map(|point| std::mem::replace(point, ev.position))
                    });
                    if let Some(from) = from {
                        let delta = (
                            (ev.position.x - from.x).as_f32(),
                            (ev.position.y - from.y).as_f32(),
                        );
                        view.update(cx, |v, cx| v.pan_by(delta, cx));
                        window.refresh();
                    }
                }
            })
            .on_mouse_up(MouseButton::Middle, {
                let view = handle.clone();
                move |_, window, cx| {
                    view.update(cx, |v, _| v.drag = None);
                    window.refresh();
                }
            })
            .on_mouse_up_out(MouseButton::Middle, {
                let view = handle.clone();
                move |_, window, cx| {
                    view.update(cx, |v, _| v.drag = None);
                    window.refresh();
                }
            })
            .on_click({
                let view = handle.clone();
                move |ev, window, cx| {
                    if ev.click_count() == 2 {
                        view.update(cx, |v, cx| v.toggle(cx));
                        window.refresh();
                    }
                }
            });

        // The picture itself, at the viewport's offset and scale: its natural
        // size times the zoom, whatever raster is behind it.
        let (w, h) = (natural.0 * view_state.scale, natural.1 * view_state.scale);
        let mut picture = div()
            .id(("image-preview-img", slot))
            .absolute()
            .left(px(view_state.offset.0))
            .top(px(view_state.offset.1))
            .w(px(w))
            .h(px(h))
            .debug_selector(move || selector("image-preview-img", slot))
            .overflow_hidden();
        if has_alpha {
            picture = picture.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        // Clipped to the view, not the picture: the picture is
                        // as large as its zoom makes it.
                        paint_checker(bounds, view_bounds, background, muted, window);
                    },
                )
                .absolute()
                .size_full(),
            );
        }
        // An element id on the image itself is what lets gpui drive a GIF or
        // WebP's frames: animation state is kept per identified element.
        body = body.child(
            picture.child(
                img(ImageSource::Render(image))
                    .id("image-preview-pixels")
                    .size_full(),
            ),
        );

        let chip = |el: gpui::Stateful<gpui::Div>| {
            el.flex()
                .items_center()
                .h(px(24.))
                .px(px(8.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(foreground)
                .bg(chip_bg)
        };
        let pct = format!("{}%", (view_state.scale * 100.).round());
        let toolbar = v_flex()
            .id("image-preview-toolbar")
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .gap_1()
            .child(chip(div().id("image-preview-zoom")).child(pct))
            .child({
                let view = handle.clone();
                chip(div().id("image-preview-fit").cursor_pointer())
                    .hover(move |s| s.bg(chip_hover))
                    .child(t(L10nKey::ImagePreviewFit))
                    .on_click(move |_, window, cx| {
                        view.update(cx, |v, cx| v.fit(cx));
                        window.refresh();
                    })
            })
            .child({
                let view = handle.clone();
                chip(div().id("image-preview-actual").cursor_pointer().tooltip(
                    move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(t(L10nKey::ImagePreviewActualSize))
                            .build(window, cx)
                    },
                ))
                .hover(move |s| s.bg(chip_hover))
                .child("100%")
                .on_click(move |_, window, cx| {
                    view.update(cx, |v, cx| v.actual_size(cx));
                    window.refresh();
                })
            });
        let info = v_flex()
            .id("image-preview-info")
            .absolute()
            .left(px(8.))
            .bottom(px(8.))
            .max_w(px((area.0 - 16.).max(0.)))
            .gap_1()
            .children(info.into_iter().map(|line| {
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .text_color(foreground)
                    .bg(chip_bg)
                    .child(line)
            }));
        let _ = window;
        body.child(toolbar).child(info).into_any_element()
    }

    /// The lines of the info panel: the name, the format, the size and the
    /// dimensions, then whatever the viewer had to do to the picture.
    fn info_lines(name: &str, shown: &Shown) -> Vec<SharedString> {
        let mut head = vec![
            name.to_owned(),
            shown.format.name().to_owned(),
            format!(
                "{} × {} px",
                dimension(shown.natural.0),
                dimension(shown.natural.1)
            ),
            human_size(shown.bytes),
        ];
        head.retain(|part| !part.is_empty());
        let mut lines: Vec<SharedString> = vec![head.join(" · ").into()];
        match shown.animation {
            Animation::Animated { frames } => lines.push(
                t_fmt(
                    L10nKey::ImagePreviewAnimated,
                    &[("frames", &frames.to_string())],
                )
                .into(),
            ),
            Animation::FirstFrameOnly => lines.push(t(L10nKey::ImagePreviewFirstFrame).into()),
            Animation::Still => {}
        }
        if shown.reduced {
            lines.push(
                t_fmt(
                    L10nKey::ImagePreviewReduced,
                    &[
                        ("width", &shown.pixels.0.to_string()),
                        ("height", &shown.pixels.1.to_string()),
                    ],
                )
                .into(),
            );
        }
        lines
    }

    /// The card that says why there is no picture, centred.
    fn error_card(&self, cx: &App) -> AnyElement {
        let data = self.data.read(cx);
        let Content::Failed(failure) = &data.content else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let name = data.name();
        let message = failure_message(&name, &data.path, failure);
        let path = match failure.bytes {
            Some(bytes) => format!("{} · {}", data.path.display(), human_size(bytes)),
            None => data.path.display().to_string(),
        };
        div()
            .id(("image-preview-error", self.slot))
            .debug_selector({
                let slot = self.slot;
                move || selector("image-preview-error", slot)
            })
            // The card has the view's focus like the picture does, or there
            // is nothing in the tab for a click or the panel's keys to land on
            // and a close of the last file falls through to the terminal.
            .key_context(crate::ui::code_editor::DOCUMENT_KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .child(
                h_flex()
                    .max_w(px(560.))
                    .gap_3()
                    .p_4()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary)
                    .child(div().flex_none().w(px(3.)).h_full().bg(theme.danger))
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(theme.foreground)
                                    .child(t(L10nKey::ImagePreviewCantShow)),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(theme.foreground)
                                    .child(message),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.foreground)
                                    .child(path),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// The part of `image` that is inside `clip`, or `None` when none is.
fn visible_part(image: Bounds<Pixels>, clip: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let left = image.left().as_f32().max(clip.left().as_f32());
    let top = image.top().as_f32().max(clip.top().as_f32());
    let right = image.right().as_f32().min(clip.right().as_f32());
    let bottom = image.bottom().as_f32().min(clip.bottom().as_f32());
    (right > left && bottom > top).then(|| Bounds {
        origin: gpui::point(px(left), px(top)),
        size: gpui::size(px(right - left), px(bottom - top)),
    })
}

/// The squares of the checkerboard to paint: the odd ones (column + row odd,
/// counted from the picture's own top-left corner, so the pattern does not
/// move when the picture does) that are inside `clip`, each cut to the part
/// that is. The squares are found from the visible part, not by walking the
/// whole picture: zoomed in on a 4096 x 4096 picture, the picture is tens of
/// thousands of px across and millions of squares, and only the few thousand
/// that fit the view are ever painted.
fn checker_squares(
    image: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    square: f32,
) -> impl Iterator<Item = ((i32, i32), Bounds<Pixels>)> {
    let visible = visible_part(image, clip);
    let (origin_x, origin_y) = (image.left().as_f32(), image.top().as_f32());
    let (columns, rows) = match visible {
        Some(v) => (
            ((v.left().as_f32() - origin_x) / square).floor() as i32
                ..((v.right().as_f32() - origin_x) / square).ceil() as i32,
            ((v.top().as_f32() - origin_y) / square).floor() as i32
                ..((v.bottom().as_f32() - origin_y) / square).ceil() as i32,
        ),
        None => (0..0, 0..0),
    };
    rows.flat_map(move |row| columns.clone().map(move |column| (column, row)))
        .filter(|(column, row)| (column + row) % 2 != 0)
        .filter_map(move |(column, row)| {
            let cell = Bounds {
                origin: gpui::point(
                    px(origin_x + column as f32 * square),
                    px(origin_y + row as f32 * square),
                ),
                size: gpui::size(px(square), px(square)),
            };
            visible_part(cell, visible?).map(|part| ((column, row), part))
        })
}

/// Paints the transparency checkerboard of `image`, the background and the
/// muted token in alternating squares, inside `clip` only.
fn paint_checker(
    image: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    a: gpui::Hsla,
    b: gpui::Hsla,
    window: &mut Window,
) {
    let Some(visible) = visible_part(image, clip) else {
        return;
    };
    window.paint_quad(fill(visible, a));
    for (_, square) in checker_squares(image, clip, CHECKER) {
        window.paint_quad(fill(square, b));
    }
}

impl Render for ImageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if matches!(self.data.read(cx).content, Content::Failed(_)) {
            return self.error_card(cx);
        }
        let view = cx.entity();
        // The content is built in prepaint, once the area is known: the fit
        // and the re-fit on resize depend on it, and state changed from a
        // render closure cannot ask for another frame.
        div()
            .id(("image-preview-body", self.slot))
            .debug_selector({
                let slot = self.slot;
                move || selector("image-preview-body", slot)
            })
            .relative()
            .size_full()
            .child(
                canvas(
                    {
                        let view = view.clone();
                        move |bounds, window, cx| {
                            let mut surface = Self::surface(&view, bounds, window, cx);
                            surface.prepaint_as_root(bounds.origin, bounds.size.into(), window, cx);
                            surface
                        }
                    },
                    move |bounds, mut surface, window, cx| {
                        // Capture phase, before any bubble listener can pan:
                        // the zoom is decided against the pointer first.
                        let view = view.clone();
                        window.on_mouse_event(
                            move |ev: &gpui::ScrollWheelEvent, phase, window, cx| {
                                if phase != gpui::DispatchPhase::Capture
                                    || !bounds.contains(&window.mouse_position())
                                {
                                    return;
                                }
                                let zoomed =
                                    view.update(cx, |v, cx| v.wheel(ev, bounds.origin, cx));
                                if let Some(changed) = zoomed {
                                    if changed {
                                        window.refresh();
                                    }
                                    cx.stop_propagation();
                                }
                            },
                        );
                        surface.paint(window, cx);
                    },
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_files_are_talked_about() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(812), "812 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(3 * 1024 * 1024 + 400 * 1024), "3.4 MiB");
    }

    #[test]
    fn whole_dimensions_stay_whole() {
        assert_eq!(dimension(32.0), "32");
        assert_eq!(dimension(32.5), "32.5");
        assert_eq!(dimension(1024.02), "1024");
    }

    #[test]
    fn transparency_is_noticed() {
        assert!(!has_alpha(&[1, 2, 3, 255, 4, 5, 6, 255]));
        assert!(has_alpha(&[1, 2, 3, 255, 4, 5, 6, 254]));
        assert!(!has_alpha(&[]));
    }

    #[test]
    fn every_failure_is_worded_in_the_users_language_with_the_name() {
        crate::ui::i18n::set_locale("en");
        let failure = |error| Failure {
            error,
            bytes: Some(30 * MIB),
        };
        let say = |error| failure_message("shot.png", Path::new("/x/shot.png"), &failure(error));
        for error in [
            ImageError::TooLarge { bytes: 30 * MIB },
            ImageError::TooManyPixels {
                width: 5000,
                height: 5000,
            },
            ImageError::OverBudget,
            ImageError::Corrupt,
            ImageError::Unsupported(Unsupported::Format("TIFF")),
            ImageError::Unsupported(Unsupported::Feature),
            ImageError::Unsupported(Unsupported::SvgImages),
            ImageError::NotAnImage,
        ] {
            let text = say(error.clone());
            assert!(text.contains("shot.png"), "{error:?}: {text}");
            assert!(!text.contains('{'), "{error:?} left a placeholder: {text}");
        }
        let large = say(ImageError::TooLarge { bytes: 30 * MIB });
        assert!(
            large.contains("30 MiB") && large.contains("20 MiB"),
            "{large}"
        );
        let pixels = say(ImageError::TooManyPixels {
            width: 5000,
            height: 5000,
        });
        assert!(
            pixels.contains("5000 × 5000") && pixels.contains("16"),
            "{pixels}"
        );
    }

    /// A picture at the pixel limit can be as thin as 1 px and 16 777 216 px
    /// long, and fitting it takes a scale far below the diagram's floor and
    /// below any constant floor chosen for it: no side limit is a reason to
    /// refuse it, so the view must still fit it exactly.
    #[test]
    fn a_panorama_at_the_pixel_cap_fits_exactly_whatever_its_scale() {
        let area = (548., 600.);
        for (natural, what) in [
            ((16_777_216., 1.), "16M x 1"),
            ((1., 16_777_216.), "1 x 16M"),
        ] {
            let fit = viewport::fit_scale(natural, area);
            assert!(
                fit > 0. && fit < 1e-4,
                "{what}: the fit scale is tiny: {fit}"
            );
            let limits = ImageView::limits(natural, area);
            assert!(
                limits.min <= fit,
                "{what}: the floor {} must not be above the fit {fit}",
                limits.min
            );
            for vector in [false, true] {
                let view = ImageView::fit_view(natural, vector, area);
                assert!(
                    (view.scale - fit).abs() <= fit * 1e-5,
                    "{what} (vector: {vector}): fitted at {} not {fit}",
                    view.scale
                );
                let (w, h) = (natural.0 * view.scale, natural.1 * view.scale);
                assert!(view.offset.0 >= viewport::MARGIN - 0.5, "{what}: left edge");
                assert!(
                    view.offset.1 >= viewport::MARGIN - 0.5 || h < 1.,
                    "{what}: top edge"
                );
                assert!(
                    view.offset.0 + w <= area.0 - viewport::MARGIN + 0.5,
                    "{what}: right edge"
                );
                assert!(
                    view.offset.1 + h <= area.1 - viewport::MARGIN + 0.5,
                    "{what}: bottom edge"
                );
            }
        }
    }

    /// A view with no room at all, or a picture of no size, keeps a floor
    /// that is a usable number and never makes `clamp` panic.
    #[test]
    fn an_area_with_no_room_keeps_a_usable_floor() {
        for (natural, area) in [
            ((100., 100.), (10., 10.)),
            ((100., 100.), (48., 48.)),
            ((100., 100.), (0., 0.)),
            ((0., 0.), (500., 500.)),
            ((1., 1.), (500., 500.)),
        ] {
            let limits = ImageView::limits(natural, area);
            assert!(
                limits.min.is_finite() && limits.min > 0. && limits.min <= limits.max,
                "{natural:?} in {area:?}: {limits:?}"
            );
            for vector in [false, true] {
                let view = ImageView::fit_view(natural, vector, area);
                assert!(
                    view.scale.is_finite() && view.scale > 0.,
                    "{natural:?} in {area:?}"
                );
            }
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: gpui::point(px(x), px(y)),
            size: gpui::size(px(w), px(h)),
        }
    }

    /// The independent clipping test: the checkerboard of a picture zoomed far
    /// past the view paints only what the view can show. A 4096 x 4096 picture
    /// at 8x is 32768 px across, which is millions of squares; the view holds
    /// a few thousand. Without the clip, this count is ~7 million.
    #[test]
    fn the_checker_never_paints_more_squares_than_the_view_holds() {
        let image = rect(-3000., -2000., 32768., 32768.);
        let view = rect(0., 0., 1200., 800.);
        let painted: Vec<_> = checker_squares(image, view, CHECKER).collect();
        // (1200 / 12 + 2) * (800 / 12 + 2) cells in view, half of them odd.
        let bound = ((1200. / CHECKER + 2.) * (800. / CHECKER + 2.) / 2.) as usize + 1;
        assert!(
            painted.len() <= bound,
            "{} squares for a 1200 x 800 view, at most {bound} can be seen",
            painted.len()
        );
        assert!(painted.len() > bound / 2, "and the view is not left bare");
        // Every square is inside the view.
        for (_, square) in &painted {
            assert!(square.left().as_f32() >= view.left().as_f32() - 1e-3);
            assert!(square.top().as_f32() >= view.top().as_f32() - 1e-3);
            assert!(square.right().as_f32() <= view.right().as_f32() + 1e-3);
            assert!(square.bottom().as_f32() <= view.bottom().as_f32() + 1e-3);
        }
    }

    /// The pattern belongs to the picture, not to the view: a square is odd
    /// counted from the picture's top-left, wherever the view cuts it, and
    /// the clipped part is exactly the cell's overlap with the view.
    #[test]
    fn the_checker_keeps_its_phase_from_the_pictures_corner() {
        let image = rect(-5000.3, 40.7, 20000., 9000.);
        let view = rect(10., 20., 300., 200.);
        let painted: Vec<_> = checker_squares(image, view, CHECKER).collect();
        assert!(!painted.is_empty());
        for ((column, row), part) in painted {
            assert_eq!(
                (column + row).rem_euclid(2),
                1,
                "only odd cells are painted"
            );
            let cell = rect(
                image.left().as_f32() + column as f32 * CHECKER,
                image.top().as_f32() + row as f32 * CHECKER,
                CHECKER,
                CHECKER,
            );
            let want = visible_part(cell, view).expect("the cell is in the view");
            for (got, want, what) in [
                (part.left().as_f32(), want.left().as_f32(), "left"),
                (part.top().as_f32(), want.top().as_f32(), "top"),
                (part.right().as_f32(), want.right().as_f32(), "right"),
                (part.bottom().as_f32(), want.bottom().as_f32(), "bottom"),
            ] {
                assert!((got - want).abs() < 1e-2, "cell ({column}, {row}) {what}");
            }
        }
    }

    #[test]
    fn a_picture_out_of_view_paints_no_checker() {
        let view = rect(0., 0., 400., 300.);
        for image in [
            rect(1000., 0., 500., 500.),
            rect(-600., -600., 500., 500.),
            rect(0., 400., 100., 100.),
        ] {
            assert_eq!(
                checker_squares(image, view, CHECKER).count(),
                0,
                "{image:?}"
            );
        }
        // And a picture inside the view paints its odd cells whole.
        let inside = rect(24., 24., 120., 60.);
        assert_eq!(checker_squares(inside, view, CHECKER).count(), 25);
    }

    #[test]
    fn counts_are_written_with_their_thousands_set_off() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(16_777_216), "16,777,216");
    }

    #[test]
    fn the_pixel_limit_is_named_exactly_and_an_unmeasured_size_is_not_claimed() {
        crate::ui::i18n::set_locale("en");
        let say = |error, bytes| {
            failure_message("a.png", Path::new("/x/a.png"), &Failure { error, bytes })
        };
        let pixels = say(
            ImageError::TooManyPixels {
                width: 5000,
                height: 5000,
            },
            Some(1),
        );
        assert!(pixels.contains("16,777,216"), "{pixels}");
        // A file that grew past the limit after it was measured has no size
        // to name: only the limit.
        let grew = say(
            ImageError::TooLarge {
                bytes: MAX_IMAGE_BYTES + 1,
            },
            None,
        );
        assert!(grew.contains("20 MiB"), "{grew}");
        assert!(!grew.contains("21"), "no size is claimed: {grew}");
        let measured = say(ImageError::TooLarge { bytes: 30 * MIB }, Some(30 * MIB));
        assert!(
            measured.contains("30 MiB") && measured.contains("20 MiB"),
            "{measured}"
        );
    }
}
