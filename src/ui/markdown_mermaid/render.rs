//! Drawing mermaid blocks in the markdown preview.
//!
//! [`render_block`] is the single entry the parent module's plugin calls.
//! Every diagram on screen flows through the app-global
//! [`cache::MermaidCache`]: a hit draws the cached [`gpui::RenderImage`],
//! a miss draws a placeholder (or the block's last-good image) until its
//! render lands, and a failed render draws a one-line error above the
//! original fence as an ordinary code block.
//!
//! # Visible-prepaint scheduling
//!
//! The editor preview's text view is not virtualized: every block of the
//! document is built every frame, on screen or not. A bounded cache under
//! that regime only works if offscreen blocks are inert, so building an
//! element only *reads* the cache ([`cache::MermaidCache::peek`] is
//! touch-free), and all state changes — recency touches, drawing-window
//! records, last-good stores, and the one request a miss is allowed —
//! happen in [`VisibleBlock`]'s prepaint, and only when the block's bounds
//! intersect the visible clip. An offscreen block cannot schedule renders
//! or refresh recency, which is what keeps the cache meaningful.
//!
//! # Layout
//!
//! A ready diagram is drawn at its **logical** size — device pixels divided
//! by the scale the raster came back at, which can differ from the
//! requested scale when the renderer capped it ([`MAX_PIXELS`]). The box
//! keeps the raster's aspect ratio, never grows past its intrinsic size,
//! and shrinks to the available preview width: the wrapper carries the
//! definite width, `max_w_full()` and the aspect ratio, and the `img` fills
//! it with `size_full()` (both definite, so the image element's own
//! auto-height sizing — which uses the unclamped intrinsic size — never
//! applies). The layout tests in this file assert the resulting box at
//! wide, exact and narrow widths through gpui's real layout engine.
//!
//! # Atlas ownership
//!
//! The cache hands out `Arc<RenderImage>` clones; a window that paints one
//! uploads it to that window's sprite atlas. Because the atlas is per
//! window, the cache records every window that drew a retained image, and
//! when the last reference to an image goes away it is dropped from each
//! recorded window ([`Window::drop_image`], or the window's handle for
//! other windows — a closed window simply fails its update, which is
//! ignored). All of this happens on the UI thread with a `&mut Window` in
//! hand — in `render_block`'s build pass and, for the deferred request and
//! its immediate cap enforcement, in [`VisibleBlock`]'s prepaint.
//!
//! [`MAX_PIXELS`]: tty59_mermaid::MAX_PIXELS

mod cache;

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{AnyElement, App, ImageSource, Pixels, RenderImage, Window, div, img, px};
use gpui_component::text::TextView;
use gpui_component::{ActiveTheme as _, v_flex};
use image::{Frame, RgbaImage};
use smallvec::SmallVec;
use tty59_mermaid::MermaidTheme;

use self::cache::{CacheKey, Decision, LogicalSize, MermaidCache, Release, ScheduledRender};
use super::MermaidBlock;
use crate::ui::i18n::{L10nKey, t};

/// Placeholder height while a block's first render runs, when not even a
/// last-good image gives the block a stable height of its own.
const PENDING_HEIGHT: f32 = 96.;

/// Draw one mermaid block: the cached diagram, a placeholder (or the
/// block's last-good image) while rendering, or the failure fallback.
///
/// Building the element only *reads* the cache ([`MermaidCache::peek`] is
/// touch-free): in the unvirtualized preview every block of the document is
/// built every frame, and offscreen blocks must not look recently used or
/// they would age the visible diagrams' entries out. Everything that
/// mutates recency — touching entries, recording drawing windows, storing
/// last-goods, and above all scheduling renders for misses — happens in
/// [`VisibleBlock`]'s prepaint, once the block's bounds are known to
/// intersect the visible clip.
pub(super) fn render_block(block: &MermaidBlock, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = crate::ui::theme::mermaid_theme(cx);
    let scale = window.scale_factor();
    let key = CacheKey::new(&block.source, theme.clone(), scale);

    // First use installs the production cache; tests install their own with
    // an injected renderer before the first draw.
    if !cx.has_global::<MermaidCache>() {
        cx.set_global(MermaidCache::default());
    }

    // One touch-free read decides everything.
    let decision = cx.global::<MermaidCache>().peek(&key);

    // The display for not-yet-drawable states is decided once: the block's
    // own last-good image when it matches the theme and scale, else the
    // stable-height placeholder.
    let pending = match &decision {
        Decision::Pending | Decision::Idle => Some(pending_display(
            block.block_id,
            &theme,
            key.scale_bits(),
            cx,
        )),
        Decision::Ready(..) | Decision::Failed(..) => None,
    };

    let on_visible = match (&decision, &pending) {
        (Decision::Ready(image, logical), _) => OnVisible::Ready {
            image: image.clone(),
            logical: *logical,
        },
        (Decision::Failed(_), _) => OnVisible::Failed,
        // A last-good is displayed while the source's own render is missing:
        // for a pending entry it is already in flight; for an idle block
        // (an edited source, typically) it must be scheduled now.
        (Decision::Idle, Some(PendingDisplay::Image(image, _))) => OnVisible::LastGood {
            image: image.clone(),
            schedule: true,
        },
        (Decision::Pending, Some(PendingDisplay::Image(image, _))) => OnVisible::LastGood {
            image: image.clone(),
            schedule: false,
        },
        (Decision::Pending, _) => OnVisible::Placeholder {
            schedule_on_visible: false,
        },
        (Decision::Idle, _) => OnVisible::Placeholder {
            schedule_on_visible: true,
        },
    };

    let inner = match (&decision, &pending) {
        (Decision::Ready(image, logical), _) => diagram_element(image, *logical),
        (Decision::Failed(message), _) => failure_element(block, message, cx),
        (_, Some(PendingDisplay::Image(image, logical))) => diagram_element(image, *logical),
        (_, Some(PendingDisplay::Placeholder(height))) => pending_element(Some(*height), cx),
        (Decision::Pending | Decision::Idle, None) => {
            unreachable!("pending states always carry a display")
        }
    };

    // Capacity enforcement runs here, immediately — the window is in hand —
    // and again in `VisibleBlock`'s prepaint after the deferred request.
    // Executing now matters: a release parked in a wrapper that never
    // prepaints (an offscreen block) would leak its atlas tiles.
    let live = cx.windows();
    for release in cx.global_mut::<MermaidCache>().collect_releases(&live) {
        release_on_windows(release, window, cx);
    }
    VisibleBlock {
        inner,
        key,
        block_id: block.block_id,
        on_visible,
        acted: false,
    }
    .into_any_element()
}

/// What a pending or idle block displays while its own render is not
/// drawable yet: the block's last-good image when it matches the current
/// theme and scale, or the stable-height placeholder.
enum PendingDisplay {
    Image(Arc<RenderImage>, LogicalSize),
    Placeholder(f32),
}

fn pending_display(
    block_id: u64,
    theme: &MermaidTheme,
    scale_bits: u32,
    cx: &App,
) -> PendingDisplay {
    if let Some((image, logical)) = cx
        .global::<MermaidCache>()
        .last_good_for_block(block_id, theme, scale_bits)
    {
        return PendingDisplay::Image(image, logical);
    }
    PendingDisplay::Placeholder(
        cx.global::<MermaidCache>()
            .last_good_height(block_id)
            .unwrap_or(PENDING_HEIGHT),
    )
}

/// What the cache should do the first time a block's element reaches
/// *visible* prepaint.
#[derive(Clone)]
enum OnVisible {
    /// Displaying a ready diagram: record the drawing window and store the
    /// block's last-good. Done here, not at build time, so offscreen ready
    /// blocks cannot churn the last-good map.
    Ready {
        image: Arc<RenderImage>,
        logical: LogicalSize,
    },
    /// Displaying the block's last-good image: record the drawing window
    /// and touch the last-good's recency. `schedule` is set when the
    /// block's own source has no entry yet (an edited source riding its
    /// last-good): its render is requested at visible prepaint.
    LastGood {
        image: Arc<RenderImage>,
        schedule: bool,
    },
    /// Displaying a placeholder. `schedule_on_visible` is set when nothing
    /// has been scheduled yet (the block went straight from idle to
    /// visible); visible prepaint then performs the one request per miss.
    Placeholder { schedule_on_visible: bool },
    /// Displaying the failure fallback: only the entry's recency needs a
    /// touch.
    Failed,
}

/// The wrapper every mermaid block's element goes through. Layout is a
/// plain pass-through (auto-sized around the inner element), so the
/// diagram's max-width/aspect constraints apply unchanged. At prepaint,
/// when this block's bounds are known, one of two things happens:
///
/// - the bounds intersect the visible clip: the block is actually being
///   shown, so its entry's recency is touched, ready and last-good
///   displays record their drawing windows, a ready display stores the
///   block's last-good, and a never-scheduled miss finally requests its
///   render — followed by immediate capacity enforcement, since the
///   build-time pass ran before this insertion existed;
/// - the bounds are offscreen: nothing at all. An offscreen block cannot
///   schedule renders or refresh recency, which is what keeps a bounded
///   cache meaningful under the unvirtualized preview.
struct VisibleBlock {
    inner: AnyElement,
    key: CacheKey,
    block_id: u64,
    on_visible: OnVisible,
    acted: bool,
}

impl gpui::Element for VisibleBlock {
    type RequestLayoutState = gpui::LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, gpui::LayoutId) {
        // Forward the inner element's layout node directly — no wrapper
        // node of its own. A default-style node here would stretch the
        // block to the parent's full width, overriding the inner diagram's
        // width/max-width/aspect constraints; forwarding preserves the
        // child's geometry exactly, and prepaint/paint read the same id.
        let child = self.inner.request_layout(window, cx);
        (child, child)
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _bounds: gpui::Bounds<Pixels>,
        child: &mut gpui::LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.prepaint(window, cx);
        let child_bounds = window.layout_bounds(*child);

        if self.acted {
            return;
        }
        self.acted = true;

        let visible = {
            let clip = window.content_mask().bounds;
            let hit = child_bounds.intersect(&clip);
            hit.size.width > px(0.) && hit.size.height > px(0.)
        };
        if !visible {
            return;
        }

        // Snapshot the live windows before borrowing the cache: the
        // release path below needs both, and they must not alias.
        let live = cx.windows();
        let handle = window.window_handle();
        cx.global_mut::<MermaidCache>().touch(&self.key);
        let mut decided: Vec<Release> = Vec::new();
        match &self.on_visible {
            OnVisible::Ready { image, logical } => {
                let cache = cx.global_mut::<MermaidCache>();
                cache.record_window(image, handle, &live);
                decided.extend(cache.store_last_good(
                    self.block_id,
                    self.key.theme().clone(),
                    self.key.scale_bits(),
                    image.clone(),
                    *logical,
                ));
            }
            OnVisible::LastGood { image, schedule } => {
                let cache = cx.global_mut::<MermaidCache>();
                cache.record_window(image, handle, &live);
                cache.touch_last_good(self.block_id);
                if *schedule {
                    let (_, task) = cache.request(&self.key);
                    if let Some(task) = task {
                        spawn_render(task, cx);
                    }
                }
            }
            OnVisible::Placeholder {
                schedule_on_visible: true,
            } => {
                let (_, task) = cx.global_mut::<MermaidCache>().request(&self.key);
                if let Some(task) = task {
                    spawn_render(task, cx);
                }
            }
            OnVisible::Placeholder {
                schedule_on_visible: false,
            }
            | OnVisible::Failed => {}
        }

        // Enforce the caps right after the deferred insertion (and the
        // last-good store above): the build-time collection ran before
        // either existed.
        decided.extend(cx.global_mut::<MermaidCache>().collect_releases(&live));
        for release in decided {
            release_on_windows(release, window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        _: &mut gpui::LayoutId,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(window, cx);
    }
}

impl gpui::IntoElement for VisibleBlock {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Run one scheduled render on the background executor and marshal the
/// result back into the cache, refreshing every window afterwards — the
/// same diagram may be displayed in several of them.
fn spawn_render(task: ScheduledRender, cx: &App) {
    let ScheduledRender {
        key,
        source,
        theme,
        scale,
        generation,
        renderer,
    } = task;
    cx.spawn(async move |cx| {
        let raster = cx
            .background_spawn(async move { renderer(&source, &theme, scale) })
            .await;
        cx.update(move |cx| {
            // The cache can be gone in tests; a dropped completion is
            // harmless.
            if cx.has_global::<MermaidCache>() {
                cx.global_mut::<MermaidCache>()
                    .complete(&key, generation, raster);
                cx.refresh_windows();
            }
        });
    })
    .detach();
}

/// Drop a released image from the sprite atlas of every window that drew
/// it. The window currently rendering is updated directly; others go
/// through their handles, where a closed window reports an error that is
/// simply ignored — its atlas is gone with it. Returns how many live
/// windows the image was dropped from.
fn release_on_windows(release: Release, window: &mut Window, cx: &mut App) -> usize {
    let Release { image, windows } = release;
    let current = window.window_handle();
    let mut dropped = 0;
    for handle in windows {
        if handle == current {
            let _ = window.drop_image(image.clone());
            dropped += 1;
        } else if handle
            .update(cx, |_, window, _| window.drop_image(image.clone()))
            .is_ok()
        {
            dropped += 1;
        }
    }
    dropped
}

/// A ready diagram: intrinsic logical width (never upscaled), shrunk to the
/// available width, aspect kept by the wrapper's `aspect_ratio`.
fn diagram_element(image: &Arc<RenderImage>, logical: LogicalSize) -> AnyElement {
    // Degenerate sizes are clamped so the ratio stays positive even if a
    // renderer returns something extreme.
    let width = logical.width.max(1.0);
    let height = logical.height.max(1.0);
    div()
        .w(px(width))
        .max_w_full()
        .aspect_ratio(width / height)
        .child(img(ImageSource::Render(image.clone())).size_full())
        .into_any_element()
}

/// The placeholder shown while a block renders: a stable-height box with
/// the localized "rendering" text. `height` is the block's last-good height
/// when known, so the layout does not jump between placeholder and image.
fn pending_element(height: Option<f32>, cx: &App) -> AnyElement {
    let height = height
        .filter(|height| height.is_finite() && *height > 0.0)
        .unwrap_or(PENDING_HEIGHT);
    let theme = cx.theme();
    div()
        .w_full()
        .h(px(height))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .text_color(theme.muted_foreground)
        .text_size(theme.font_size)
        .child(t(L10nKey::MermaidRendering))
        .into_any_element()
}

/// A failed render: one localized error line in the theme's danger color
/// above the original fence, rendered as an ordinary code block by a
/// nested text view without this plugin — the fallback cannot recurse.
fn failure_element(block: &MermaidBlock, message: &str, cx: &App) -> AnyElement {
    v_flex()
        .gap_1()
        .child(error_line(message, cx))
        .child(
            TextView::markdown(
                ("mermaid-fallback", block.block_id),
                block.original_fence.clone(),
            )
            .style(crate::ui::theme::markdown_style(cx)),
        )
        .into_any_element()
}

/// The single error line above the fallback code block. An empty detail
/// (nothing usable came back from the renderer) renders the localized
/// label alone rather than any hardcoded English text.
fn error_line(message: &str, cx: &App) -> AnyElement {
    let label = t(L10nKey::MermaidRenderError);
    let text = if message.is_empty() {
        label.to_string()
    } else {
        format!("{label}: {message}")
    };
    div()
        .text_color(cx.theme().danger)
        .text_size(cx.theme().font_size)
        .truncate()
        .child(text)
        .into_any_element()
}

/// Consume the renderer contract's straight-BGRA raster, turning it into a
/// gpui [`RenderImage`], with the logical size the layout must use: device
/// pixels divided by the scale the raster *reports* (which may be lower
/// than requested after the renderer's pixel cap), never the requested
/// scale. Malformed rasters — zero dimensions, a byte count that does not
/// match, or a non-finite/non-positive scale — are rejected defensively
/// rather than trusted; the caller turns rejection into a failure state.
///
/// The raster is owned here so its pixel buffer can be handed to the image
/// (see `raster_conversion_moves_the_buffer_without_cloning`).
fn raster_to_image(raster: tty59_mermaid::Raster) -> Option<(Arc<RenderImage>, LogicalSize)> {
    let width = raster.width;
    let height = raster.height;
    if width == 0 || height == 0 {
        return None;
    }
    let expected = (width as u64).checked_mul(height as u64)?.checked_mul(4)?;
    if expected != raster.bgra.len() as u64 {
        return None;
    }
    let scale = raster.scale;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    // The logical size must also be a usable finite number: a tiny positive
    // scale divides the pixel size into infinity, and that must not reach
    // layout (or the aspect ratio).
    let logical = LogicalSize {
        width: width as f32 / scale,
        height: height as f32 / scale,
    };
    if !logical.width.is_finite()
        || !logical.height.is_finite()
        || logical.width <= 0.0
        || logical.height <= 0.0
    {
        return None;
    }
    // The raster is already straight-alpha BGRA — exactly the layout
    // `RenderImage` frames upload — so the buffer moves into the image
    // without a copy: `RgbaImage::from_raw` takes the vector verbatim.
    let buffer = RgbaImage::from_raw(width, height, raster.bgra)?;
    let frame = Frame::new(buffer);
    // `from_buf` moves the frame into inline storage. `from_elem(frame, 1)`
    // would loop `elem.clone()`, cloning the frame and its pixel buffer even
    // for one element — which is exactly the copy this converter avoids.
    let image = Arc::new(RenderImage::new(SmallVec::from_buf([frame])));
    Some((image, logical))
}

/// Collapse a renderer message to the one line the fallback shows: first
/// non-empty line, trimmed, capped. Nothing usable comes back as an empty
/// string, which [`error_line`] renders as the localized label alone — no
/// untranslated fallback text exists here.
fn failure_message(raw: &str) -> String {
    const CAP: usize = 200;
    let first = raw.lines().find(|line| !line.trim().is_empty());
    let mut text = String::new();
    if let Some(first) = first {
        for (i, ch) in first.trim().chars().enumerate() {
            if i >= CAP {
                text.push('…');
                break;
            }
            text.push(ch);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::cache::{CAP_ENTRIES, CAP_LAST_GOOD, RenderFn};
    use super::*;
    use gpui::{
        App, AvailableSpace, Bounds, Element, EntityId, GlobalElementId, InspectorElementId,
        IntoElement, LayoutId, Pixels, Point, Style, TestAppContext, VisualTestContext, Window,
        div, point, px, size,
    };
    use gpui_component::{Theme, ThemeMode};
    use std::sync::Mutex;
    use tty59_mermaid::{MermaidError, MermaidTheme};

    /// The test platform's windows report a 2× scale factor; cache keys in
    /// tests are built with it so they hit.
    const TEST_SCALE: f32 = 2.0;

    fn raster(width: u32, height: u32, scale: f32) -> tty59_mermaid::Raster {
        tty59_mermaid::Raster {
            width,
            height,
            scale,
            bgra: vec![0x20; width as usize * height as usize * 4],
        }
    }

    fn block(id: u64, source: &str) -> MermaidBlock {
        let original_fence = format!("```mermaid\n{source}\n```");
        MermaidBlock {
            source: source.to_string(),
            original_fence,
            block_id: id,
        }
    }

    fn ok_renderer(width: u32, height: u32, scale: f32) -> RenderFn {
        Arc::new(move |_: &str, _: &MermaidTheme, _: f32| Ok(raster(width, height, scale)))
    }

    fn key_for(cx: &App, source: &str) -> CacheKey {
        CacheKey::new(source, crate::ui::theme::mermaid_theme(cx), TEST_SCALE)
    }

    /// A window root that renders nothing; tests draw elements directly.
    struct Blank;

    impl gpui::Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// A root view that renders one mermaid block inside the measuring
    /// wrapper. The failure fallback nests a whole text view, which needs a
    /// real rendered entity on the element stack (`window.current_view()`);
    /// a raw `VisualTestContext::draw` has none, so this host draws the
    /// block the way the actual preview would.
    struct BlockHost {
        block: MermaidBlock,
        recorded: Arc<Mutex<Option<Bounds<Pixels>>>>,
    }

    impl gpui::Render for BlockHost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let inner = render_block(&self.block, window, cx);
            Measured {
                inner,
                recorded: self.recorded.clone(),
            }
        }
    }

    /// Wraps one element and records the bounds layout gave it, standing in
    /// for the markdown document's column: full available width, own
    /// height.
    struct Measured {
        inner: AnyElement,
        recorded: Arc<Mutex<Option<Bounds<Pixels>>>>,
    }

    impl Element for Measured {
        type RequestLayoutState = LayoutId;
        type PrepaintState = ();

        fn id(&self) -> Option<gpui::ElementId> {
            None
        }

        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, LayoutId) {
            let child = self.inner.request_layout(window, cx);
            let mut style = Style::default();
            style.size.width = gpui::relative(1.).into();
            (window.request_layout(style, Some(child), cx), child)
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            child: &mut LayoutId,
            window: &mut Window,
            cx: &mut App,
        ) {
            self.inner.prepaint(window, cx);
            let bounds = window.layout_bounds(*child);
            *self.recorded.lock().unwrap() = Some(bounds);
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut LayoutId,
            _: &mut (),
            window: &mut Window,
            cx: &mut App,
        ) {
            self.inner.paint(window, cx);
        }
    }

    impl IntoElement for Measured {
        type Element = Self;

        fn into_element(self) -> Self {
            self
        }
    }

    /// Draw `build` as the root of the test window with a definite
    /// available size, and return the bounds layout gave the wrapped
    /// element.
    fn draw_measured(
        cx: &mut VisualTestContext,
        available: gpui::Size<Pixels>,
        build: impl FnOnce(&mut Window, &mut App) -> AnyElement,
    ) -> Bounds<Pixels> {
        let recorded: Arc<Mutex<Option<Bounds<Pixels>>>> = Arc::new(Mutex::new(None));
        let sink = recorded.clone();
        cx.draw(
            Point::default(),
            size(
                AvailableSpace::Definite(available.width),
                AvailableSpace::Definite(available.height),
            ),
            move |window, cx| Measured {
                inner: build(window, cx),
                recorded: sink,
            },
        );
        recorded
            .lock()
            .unwrap()
            .take()
            .expect("prepaint records the child bounds")
    }

    fn install(cx: &mut App, renderer: RenderFn) {
        gpui_component::init(cx);
        cx.set_global(MermaidCache::new(renderer));
    }

    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 0.02,
            "{what}: expected {expected}, got {actual}"
        );
    }

    /// The rendered diagram keeps its aspect ratio, shrinks to the
    /// available width and never upscales — through gpui's real layout.
    /// The raster's own scale (4×) differs from the window's requested
    /// scale (2×), so a layout that used the requested scale instead of
    /// the raster's would be visibly wrong here.
    #[gpui::test]
    fn layout_fits_shrinks_and_never_upscales(cx: &mut TestAppContext) {
        // 400×200 device pixels at a raster scale of 4: logical size
        // 100×50. (Dividing by the requested 2× would wrongly give 200×100.)
        cx.update(|cx| install(cx, ok_renderer(400, 200, 4.0)));
        let vcx = cx.add_empty_window();
        let drawn = block(1, "graph TD\nA-->B");

        // First draw is pending; park to let the render land, then draw the
        // hit.
        draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                cx.global::<MermaidCache>()
                    .state_of(&key_for(cx, "graph TD\nA-->B")),
                Some("ready")
            );
        });

        // Wide preview: intrinsic logical size — neither the 400px device
        // size nor the 200px a requested-scale division would give.
        let wide = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_close(f32::from(wide.size.width), 100.0, "intrinsic width");
        assert_close(
            f32::from(wide.size.height),
            50.0,
            "intrinsic height keeps the ratio",
        );

        // Narrower than the diagram: shrink to the available width with the
        // aspect kept.
        let narrow = draw_measured(vcx, size(px(75.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_close(f32::from(narrow.size.width), 75.0, "clamped width");
        assert_close(
            f32::from(narrow.size.height),
            37.5,
            "height follows the clamp",
        );

        // Much wider than the diagram: never upscaled.
        let huge = draw_measured(vcx, size(px(2000.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_close(f32::from(huge.size.width), 100.0, "no upscaling");
        assert_close(f32::from(huge.size.height), 50.0, "no upscaling (height)");
    }

    /// While a block re-renders (source edit) its last-good image stays on
    /// screen; a theme change does not reuse the old palette's image.
    #[gpui::test]
    fn pending_shows_last_good_but_not_across_themes(cx: &mut TestAppContext) {
        cx.update(|cx| install(cx, ok_renderer(100, 50, 1.0)));
        let vcx = cx.add_empty_window();

        // First render of block 7: placeholder at the default height.
        let first = block(7, "graph TD\nA-->B");
        let pending = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&first, window, cx)
        });
        assert_close(
            f32::from(pending.size.height),
            PENDING_HEIGHT,
            "placeholder height",
        );
        vcx.run_until_parked();
        // Draw the hit once more so the block actually stores its last-good.
        draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&first, window, cx)
        });

        // Now an edit (pending again) shows that image, not a placeholder:
        // 100×50 logical (the raster's own scale).
        let edited = block(7, "graph TD\nA-->B2");
        let stable = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&edited, window, cx)
        });
        assert_close(f32::from(stable.size.width), 100.0, "last-good width");
        assert_close(f32::from(stable.size.height), 50.0, "last-good height");
        vcx.run_until_parked();

        // A theme change re-renders (new key) and must not show the old
        // palette's image while that runs: back to the placeholder box —
        // full width, at the block's stable last-good height — instead of
        // the 100×50 old-palette image.
        vcx.update(|_, cx| Theme::change(ThemeMode::Dark, None, cx));
        let dark = block(7, "graph TD\nA-->B3");
        let themed = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&dark, window, cx)
        });
        assert_close(
            f32::from(themed.size.width),
            400.0,
            "no last-good image across a theme change",
        );
        assert_close(
            f32::from(themed.size.height),
            50.0,
            "stable last-good height",
        );
        // The dark render did land, under the new palette's key.
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let dark_theme = crate::ui::theme::mermaid_theme(cx);
            let key = CacheKey::new("graph TD\nA-->B3", dark_theme, TEST_SCALE);
            assert_eq!(cx.global::<MermaidCache>().state_of(&key), Some("ready"));
        });
    }

    /// The colors toggle is a live palette change, driven through
    /// `render_block` exactly as the preview drives it. Turning the setting
    /// off re-keys the block (accents are part of the theme), schedules
    /// exactly one new render under the neutral palette, and — asserted
    /// before that render is allowed to settle — the pending state shows
    /// the stable-height placeholder, never the colored image this block
    /// showed a moment ago. Turning the setting back on finds the colored
    /// entry still cached and reuses it without a third render. The
    /// renderer records each request's accent count and draws a different
    /// footprint per palette, so the on-screen geometry says which
    /// palette's image is actually up.
    #[gpui::test]
    fn mermaid_colors_toggle_rerenders_and_never_shows_the_stale_last_good(
        cx: &mut TestAppContext,
    ) {
        let requests: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let renderer: RenderFn = Arc::new(move |_, theme, _| {
            let accents = theme.accents.len();
            seen.lock().unwrap().push(accents);
            // Colored: 100×50 logical. Neutral: 120×60 — one footprint per
            // palette, so width alone identifies the image on screen.
            if accents == 0 {
                Ok(raster(120, 60, 1.0))
            } else {
                Ok(raster(100, 50, 1.0))
            }
        });
        cx.update(|cx| install(cx, renderer));
        let vcx = cx.add_empty_window();
        let source = "graph TD\nA-->B";
        let drawn = block(9, source);

        // No Config global: colors default on. First render lands, is shown
        // once more so the block stores its colored last-good.
        let pending = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_close(
            f32::from(pending.size.height),
            PENDING_HEIGHT,
            "first draw is the placeholder",
        );
        vcx.run_until_parked();
        draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_eq!(
            *requests.lock().unwrap(),
            vec![6],
            "one colored render so far"
        );

        // Toggle off. The next draw is the neutral key's pending state;
        // assert it before parking, while the request is still in flight.
        vcx.update(|_, cx| {
            cx.set_global(crate::core::config::Config(
                crate::core::config::CoreConfig {
                    editor_mermaid_colors: false,
                    ..Default::default()
                },
            ));
        });
        let neutral_pending = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_close(
            f32::from(neutral_pending.size.width),
            400.0,
            "no colored last-good while the neutral render is pending",
        );
        assert_close(
            f32::from(neutral_pending.size.height),
            50.0,
            "the block keeps its stable last-good height",
        );
        assert_eq!(
            *requests.lock().unwrap(),
            vec![6],
            "the neutral render is scheduled, not yet run"
        );
        vcx.update(|_, cx| {
            let key = key_for(cx, source);
            let cache = cx.global::<MermaidCache>();
            assert_eq!(
                cache.state_of(&key),
                Some("pending"),
                "exactly one neutral request, already in flight"
            );
            assert!(
                cache
                    .last_good_for_block(9, key.theme(), key.scale_bits())
                    .is_none(),
                "the colored last-good must not match the neutral theme"
            );
        });

        // The neutral render lands with its own footprint…
        vcx.run_until_parked();
        assert_eq!(
            *requests.lock().unwrap(),
            vec![6, 0],
            "the neutral render ran with an empty palette"
        );
        let neutral = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_close(
            f32::from(neutral.size.width),
            120.0,
            "the neutral image, identified by its own size",
        );
        assert_close(f32::from(neutral.size.height), 60.0, "neutral height");

        // …and toggling back on reuses the still-cached colored entry: no
        // third render, the colored footprint back on screen.
        vcx.update(|_, cx| {
            cx.global_mut::<crate::core::config::Config>()
                .editor_mermaid_colors = true;
        });
        let colored_again = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
            render_block(&drawn, window, cx)
        });
        assert_close(
            f32::from(colored_again.size.width),
            100.0,
            "the cached colored entry is reused",
        );
        assert_close(f32::from(colored_again.size.height), 50.0, "colored height");
        assert_eq!(
            *requests.lock().unwrap(),
            vec![6, 0],
            "no re-render for a palette that is still cached"
        );
    }

    /// With Part A's real renderer, an invalid source takes the failure
    /// path: the error line above the original source as an ordinary code
    /// block, visibly taller than the error line alone. The draw goes
    /// through a real rendered view — the fallback nests a text view, which
    /// needs an entity on the element stack just like the actual preview
    /// provides. The source is deliberately not valid mermaid.
    #[gpui::test]
    fn invalid_source_failure_renders_the_fallback(cx: &mut TestAppContext) {
        cx.update(|cx| gpui_component::init(cx));
        let source = "definitely not a diagram: ((( ]][";
        let recorded: Arc<Mutex<Option<Bounds<Pixels>>>> = Arc::new(Mutex::new(None));
        let (_, vcx) = cx.add_window_view(|_, _| BlockHost {
            block: block(3, source),
            recorded: recorded.clone(),
        });

        // First draw renders the placeholder and schedules the render.
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                cx.global::<MermaidCache>().state_of(&key_for(cx, source)),
                Some("failed")
            );
        });

        // The failure draw goes through the same view, so the nested text
        // view renders in a real preview-shaped element stack.
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let full = recorded
            .lock()
            .unwrap()
            .take()
            .expect("the host view records its bounds");

        // Reference: the error line alone, which has no nested view and
        // can be drawn raw.
        let message =
            failure_message(&MermaidError::Syntax("invalid diagram source".into()).to_string());
        let line_only = draw_measured(vcx, size(px(400.), px(2000.)), |_, cx| {
            error_line(&message, cx)
        });
        assert!(
            full.size.height > line_only.size.height + px(20.),
            "the fallback code block renders below the error line: full {:?} vs line {:?}",
            full.size,
            line_only.size
        );
    }

    /// The same source in two blocks of one window renders once and both
    /// show the same image; the drawing record holds the window once.
    #[gpui::test]
    fn same_source_in_two_blocks_renders_once(cx: &mut TestAppContext) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        cx.update(|cx| {
            install(
                cx,
                Arc::new(move |_: &str, _: &MermaidTheme, _: f32| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(raster(100, 50, 1.0))
                }),
            )
        });
        let vcx = cx.add_empty_window();
        let left = block(1, "graph TD\nA-->B");
        let right = block(2, "graph TD\nA-->B");

        for blk in [&left, &right] {
            draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
                render_block(blk, window, cx)
            });
        }
        vcx.run_until_parked();
        for blk in [&left, &right] {
            let bounds = draw_measured(vcx, size(px(400.), px(2000.)), |window, cx| {
                render_block(blk, window, cx)
            });
            assert_close(
                f32::from(bounds.size.width),
                100.0,
                "shared render's logical width",
            );
        }
        vcx.update(|_, cx| {
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            let image = cx
                .global::<MermaidCache>()
                .ready_image(&key_for(cx, "graph TD\nA-->B"))
                .expect("ready after parking");
            let windows = cx.global::<MermaidCache>().recorded_windows(&image);
            assert_eq!(windows.len(), 1, "one window recorded once");
        });
    }

    /// A markdown-preview root view for the churn regressions: renders a
    /// fixed document through the real plugin pipeline, counts its own
    /// renders, and holds one stable extension set for its lifetime (a
    /// clone is installed per render), isolating cache churn from
    /// registration churn (B4).
    struct SettlePreview {
        markdown: String,
        extensions: gpui_component::text::MarkdownExtensions,
        renders: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl gpui::Render for SettlePreview {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            self.renders
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            TextView::markdown("settle-preview", self.markdown.clone())
                .markdown_extensions(self.extensions.clone())
        }
    }

    fn settle_extensions() -> gpui_component::text::MarkdownExtensions {
        gpui_component::text::MarkdownExtensions::default().plugin(
            crate::ui::markdown_mermaid::MermaidPlugin::new(
                crate::ui::markdown_mermaid::PreviewIdentity::from_buffer(EntityId::from(7), 0),
            ),
        )
    }

    /// Run at most `budget` queued tasks, stopping early when idle. The
    /// churn feedback (completion → refresh → redraw → miss → spawn) makes
    /// `run_until_parked` unbounded — including the one the `#[gpui::test]`
    /// macro runs at teardown — so the churn regressions never park.
    fn drain_bounded(cx: &TestAppContext, budget: usize) {
        for _ in 0..budget {
            if !cx.background_executor.tick() {
                break;
            }
        }
    }

    /// Cold-cache churn regression: the editor preview's text view is not
    /// virtualized (`document.rs` `render_root(None)` renders every block),
    /// so a document with more distinct diagrams than the entry cap would
    /// churn the bounded cache forever if every built block requested its
    /// render — the first block's miss evicts the second block's ready
    /// entry before it is read, every block misses again on every pass, and
    /// nothing ever settles. With request deferral, the diagrams at the top
    /// end up Ready and actually drawn, offscreen misses are not scheduled,
    /// and unchanged redraws stop rendering and stop rebuilding the view.
    ///
    /// Evidence is gathered without asserting, the view is calmed (empty
    /// extensions, so no mermaid block can schedule anything) with an
    /// explicit notify, and only then do the asserts run — so every
    /// outcome, pass or fail, leaves the executor quiet for the macro's
    /// teardown park.
    #[gpui::test]
    fn a_document_with_more_diagrams_than_the_cache_settles(cx: &mut TestAppContext) {
        const PASSES: usize = 5;
        const VISIBLE_TOP: usize = 2;
        const OFFSCREEN_TAIL: usize = 20;
        const OFFSCREEN_SCHEDULED_MAX: usize = 10;
        const TICK_BUDGET: usize = 4 * (CAP_ENTRIES + 1) + 64;
        const RENDERS_MAX: usize = 32;

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let render_counter = renders.clone();
        cx.update(|cx| {
            install(
                cx,
                Arc::new(move |_: &str, _: &MermaidTheme, _: f32| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(raster(100, 50, 1.0))
                }),
            )
        });

        // One more distinct diagram than the entry cap, as a real document.
        let mut markdown = String::new();
        let total = CAP_ENTRIES + 1;
        for i in 0..total {
            markdown.push_str(&format!("```mermaid\ngraph settle {i}\n```\n\n"));
        }

        // Open the window without parking (the churn feedback would make
        // that unbounded) and draw manually.
        let handle = cx.open_window(size(px(600.), px(800.)), |_, _| SettlePreview {
            markdown,
            extensions: settle_extensions(),
            renders: render_counter,
        });
        let vcx = VisualTestContext::from_window(*handle, cx).into_mut();

        for _ in 0..PASSES {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            drain_bounded(cx, TICK_BUDGET);
        }

        // Gather evidence without asserting: the diagrams at the very top
        // are visible in any viewport and must be Ready AND actually drawn
        // (a live drawing record, not just a cached state); the far tail is
        // offscreen in any reasonable viewport and must not be fully
        // scheduled.
        let top: Vec<Option<bool>> = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            (0..VISIBLE_TOP)
                .map(|i| {
                    let key = key_for(cx, &format!("graph settle {i}"));
                    cache
                        .ready_image(&key)
                        .map(|image| cache.has_live_record(&image))
                })
                .collect()
        });
        let tail_scheduled = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            (total - OFFSCREEN_TAIL..total)
                .filter(|i| {
                    cache
                        .state_of(&key_for(cx, &format!("graph settle {i}")))
                        .is_some()
                })
                .count()
        });
        let settled_renders = calls.load(std::sync::atomic::Ordering::SeqCst);

        // Two more unchanged draws: no renders and no view rebuilds may
        // come of them.
        for _ in 0..2 {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            drain_bounded(cx, TICK_BUDGET);
        }
        let final_renders = calls.load(std::sync::atomic::Ordering::SeqCst);
        let view_rebuilds = renders.load(std::sync::atomic::Ordering::SeqCst);

        // Calm the view before returning: with no mermaid blocks left, no
        // render can be scheduled, the remaining feedback dies out under a
        // bounded drain, and the test macro's teardown park stays bounded.
        // The notify is explicit: swapping the extensions alone must not
        // depend on some other completion leaving the root dirty.
        let _ = handle.update(cx, |view, _, cx| {
            view.extensions = gpui_component::text::MarkdownExtensions::default();
            cx.notify();
        });
        drain_bounded(cx, TICK_BUDGET);

        // Evidence first: a failing assert must not hide the rest.
        eprintln!(
            "churn (cold): top={top:?} tail_scheduled={tail_scheduled}/{OFFSCREEN_TAIL} \
             renders_before_settle={settled_renders} renders_after_settle={final_renders} \
             view_rebuilds={view_rebuilds}",
        );

        for (i, state) in top.iter().enumerate() {
            assert_eq!(
                *state,
                Some(true),
                "visible diagram {i} must be Ready and actually drawn"
            );
        }
        assert!(
            tail_scheduled <= OFFSCREEN_SCHEDULED_MAX,
            "offscreen misses must not all be scheduled: {tail_scheduled}/{OFFSCREEN_TAIL}"
        );
        assert_eq!(
            final_renders, settled_renders,
            "unchanged redraws must stop re-rendering diagrams"
        );
        assert!(
            view_rebuilds <= RENDERS_MAX,
            "unchanged redraws must stop rebuilding the view: {view_rebuilds} renders"
        );
    }

    /// Prewarmed-tail regression: a cache already full of ready diagrams for
    /// a document's offscreen tail must not starve the visible diagrams at
    /// the top. If building an element refreshed recency, every offscreen
    /// block's build-time hit would look newer than any visible block's,
    /// and each visible miss would evict a *visible* diagram's entry —
    /// forever. With touch-free peeks and visible-only touches, the
    /// eviction victims are the stalest offscreen entries.
    ///
    /// Same bounded discipline as the cold regression: no parking, evidence
    /// gathered un-asserted, view calmed with an explicit notify before the
    /// asserts.
    #[gpui::test]
    fn a_prewarmed_tail_does_not_starve_visible_diagrams(cx: &mut TestAppContext) {
        const PASSES: usize = 4;
        const VISIBLE_TOP: usize = 2;
        const COLD: usize = 2;
        const TAIL_MIN_READY: usize = 60;
        const TICK_BUDGET: usize = 4 * (CAP_ENTRIES + 2) + 64;
        const RENDERS_MAX: usize = 32;

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let render_counter = renders.clone();
        cx.update(|cx| {
            install(
                cx,
                Arc::new(move |_: &str, _: &MermaidTheme, _: f32| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(raster(100, 50, 1.0))
                }),
            )
        });

        // `COLD` visible sources at the top, then exactly CAP_ENTRIES tail
        // sources the cache will be prewarmed with.
        let total = COLD + CAP_ENTRIES;
        let mut markdown = String::new();
        for i in 0..total {
            markdown.push_str(&format!("```mermaid\ngraph warm {i}\n```\n\n"));
        }

        // Prewarm the tail one source at a time — request, then a bounded
        // drain per source — so every completion lands with its own LRU
        // stamp. (Batching the requests would leave all completions with
        // one shared stamp, making the eviction victim among them random;
        // sequential stamps are deterministic: first prewarmed = stalest.)
        // Reverse document order puts the stalest entries deepest
        // offscreen: if eviction needs a victim, it takes the far tail,
        // never a block near the visible top.
        for i in (COLD..total).rev() {
            cx.update(|cx| {
                let key = key_for(cx, &format!("graph warm {i}"));
                let (_, task) = cx.global_mut::<MermaidCache>().request(&key);
                if let Some(task) = task {
                    spawn_render(task, cx);
                }
            });
            drain_bounded(cx, TICK_BUDGET);
        }
        let prewarmed = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            (COLD..total)
                .filter(|i| {
                    cache.state_of(&key_for(cx, &format!("graph warm {i}"))) == Some("ready")
                })
                .count()
        });
        assert_eq!(
            prewarmed, CAP_ENTRIES,
            "prewarm setup must complete every tail render before the view opens"
        );

        let handle = cx.open_window(size(px(600.), px(800.)), |_, _| SettlePreview {
            markdown,
            extensions: settle_extensions(),
            renders: render_counter,
        });
        let vcx = VisualTestContext::from_window(*handle, cx).into_mut();

        for _ in 0..PASSES {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            drain_bounded(cx, TICK_BUDGET);
        }

        // Evidence: the visible top must be Ready and drawn; the tail must
        // have kept (almost) all of its prewarmed entries — the only
        // legitimate victims are the stalest offscreen ones.
        let top: Vec<Option<bool>> = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            (0..VISIBLE_TOP)
                .map(|i| {
                    let key = key_for(cx, &format!("graph warm {i}"));
                    cache
                        .ready_image(&key)
                        .map(|image| cache.has_live_record(&image))
                })
                .collect()
        });
        let tail_ready = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            (COLD..total)
                .filter(|i| {
                    cache.state_of(&key_for(cx, &format!("graph warm {i}"))) == Some("ready")
                })
                .count()
        });
        let settled_renders = calls.load(std::sync::atomic::Ordering::SeqCst);

        for _ in 0..2 {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            drain_bounded(cx, TICK_BUDGET);
        }
        let final_renders = calls.load(std::sync::atomic::Ordering::SeqCst);
        let view_rebuilds = renders.load(std::sync::atomic::Ordering::SeqCst);

        let _ = handle.update(cx, |view, _, cx| {
            view.extensions = gpui_component::text::MarkdownExtensions::default();
            cx.notify();
        });
        drain_bounded(cx, TICK_BUDGET);

        eprintln!(
            "churn (prewarm): prewarmed={prewarmed}/{CAP_ENTRIES} top={top:?} \
             tail_ready={tail_ready}/{CAP_ENTRIES} renders_before_settle={settled_renders} \
             renders_after_settle={final_renders} view_rebuilds={view_rebuilds}",
        );

        for (i, state) in top.iter().enumerate() {
            assert_eq!(
                *state,
                Some(true),
                "visible diagram {i} must be Ready and actually drawn"
            );
        }
        assert!(
            tail_ready >= TAIL_MIN_READY,
            "the prewarmed tail must be the eviction victims, not the visible \
             diagrams: {tail_ready}/{CAP_ENTRIES} still ready"
        );
        assert_eq!(
            final_renders,
            prewarmed + COLD,
            "unchanged redraws must stop re-rendering: prewarmed {prewarmed} + cold \
             {COLD} diagrams is all the rendering this document ever needs"
        );
        assert!(
            view_rebuilds <= RENDERS_MAX,
            "unchanged redraws must stop rebuilding the view: {view_rebuilds} renders"
        );
    }

    /// Scroll regression: a diagram that starts below the fold stays
    /// completely inert (never scheduled), and scrolling it into view —
    /// through a real `ScrollHandle` offset change, with no root notify —
    /// is what schedules its render. This is the end-to-end proof that
    /// visibility, not element construction, drives scheduling, including
    /// for element instances that first prepainted offscreen.
    ///
    /// Same bounded discipline: manual draws, tick-budget drains, evidence
    /// gathered un-asserted, view calmed before the asserts.
    #[gpui::test]
    fn an_offscreen_diagram_schedules_when_scrolled_into_view(cx: &mut TestAppContext) {
        const TICK_BUDGET: usize = 128;

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        cx.update(|cx| {
            install(
                cx,
                Arc::new(move |_: &str, _: &MermaidTheme, _: f32| {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(raster(100, 50, 1.0))
                }),
            )
        });

        // A tall filler paragraph, then one diagram far below the fold of
        // the small (600x400) window.
        let source = "graph scroll";
        let mut markdown = String::from("filler\n\n");
        for _ in 0..60 {
            markdown.push_str("filler line keeping the diagram below the fold\n");
        }
        markdown.push_str(&format!("\n```mermaid\n{source}\n```\n"));

        struct ScrollPreview {
            markdown: String,
            extensions: gpui_component::text::MarkdownExtensions,
            scroll: gpui::ScrollHandle,
        }
        impl gpui::Render for ScrollPreview {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                div()
                    .id("scroll-preview")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        TextView::markdown("scroll-preview-body", self.markdown.clone())
                            .markdown_extensions(self.extensions.clone()),
                    )
            }
        }

        let handle = cx.open_window(size(px(600.), px(400.)), |_, _| ScrollPreview {
            markdown,
            extensions: settle_extensions(),
            scroll: gpui::ScrollHandle::new(),
        });
        let scroll = handle
            .update(cx, |view, _, _| view.scroll.clone())
            .expect("the preview view lives");
        let vcx = VisualTestContext::from_window(*handle, cx).into_mut();

        // Below the fold: the diagram must stay completely unscheduled.
        for _ in 0..2 {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            drain_bounded(cx, TICK_BUDGET);
        }
        let offscreen = cx.update(|cx| {
            cx.global::<MermaidCache>()
                .state_of(&key_for(cx, source))
                .is_some()
        });
        let offscreen_calls = calls.load(std::sync::atomic::Ordering::SeqCst);

        // Scroll the diagram into view — offset only, no root notify. Jump
        // to the measured bottom (the diagram is the document's last
        // block), so the exact filler height never matters. The next
        // frame's prepaint sees bounds intersecting the content mask and
        // schedules the one render this block gets.
        let max = scroll.max_offset();
        scroll.set_offset(point(px(0.), -max.y));
        for _ in 0..2 {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            drain_bounded(cx, TICK_BUDGET);
        }
        let scrolled = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            cache
                .ready_image(&key_for(cx, source))
                .map(|image| cache.has_live_record(&image))
        });
        let scrolled_calls = calls.load(std::sync::atomic::Ordering::SeqCst);

        // Unchanged redraws settle: no further renders.
        for _ in 0..2 {
            vcx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            drain_bounded(cx, TICK_BUDGET);
        }
        let final_calls = calls.load(std::sync::atomic::Ordering::SeqCst);

        // Calm the view before returning (the teardown park must stay
        // bounded), then assert everything.
        let _ = handle.update(cx, |view, _, cx| {
            view.extensions = gpui_component::text::MarkdownExtensions::default();
            cx.notify();
        });
        drain_bounded(cx, TICK_BUDGET);

        eprintln!(
            "scroll: offscreen_scheduled={offscreen} offscreen_calls={offscreen_calls} \
             scrolled={scrolled:?} scrolled_calls={scrolled_calls} final_calls={final_calls}",
        );

        assert!(
            !offscreen && offscreen_calls == 0,
            "a below-the-fold diagram must stay unscheduled and unrendered"
        );
        assert_eq!(
            scrolled,
            Some(true),
            "the scrolled-in diagram must become Ready and actually drawn"
        );
        assert_eq!(
            scrolled_calls, 1,
            "the diagram renders exactly once when it scrolls into view"
        );
        assert_eq!(
            final_calls, scrolled_calls,
            "unchanged redraws after scrolling settle with no further renders"
        );
    }

    /// Drawing records track every window that showed a diagram; releasing
    /// an image drops it from every live window that drew it (the current
    /// one directly, others through their handles); a closed window's
    /// handle is pruned from the record; and aging a diagram fully out of
    /// the cache releases it through the render path.
    #[gpui::test]
    fn eviction_releases_images_across_windows(cx: &mut TestAppContext) {
        cx.update(|cx| install(cx, ok_renderer(100, 50, 1.0)));
        let first = cx.open_window(size(px(400.), px(2000.)), |_, _| Blank);
        let second = cx.open_window(size(px(400.), px(2000.)), |_, _| Blank);

        let v1 = VisualTestContext::from_window(*first, cx).into_mut();
        let v2 = VisualTestContext::from_window(*second, cx).into_mut();

        // Phase 1 — a diagram displayed by both live windows, then fully
        // aged out of the cache. The release must reach both atlases: the
        // count observed by `release_on_windows` proves the other-window
        // branch (the handle update) really ran.
        let probe = block(2, "graph release probe");
        draw_measured(v1, size(px(400.), px(2000.)), |window, cx| {
            render_block(&probe, window, cx)
        });
        cx.run_until_parked();
        draw_measured(v1, size(px(400.), px(2000.)), |window, cx| {
            render_block(&probe, window, cx)
        });
        draw_measured(v2, size(px(400.), px(2000.)), |window, cx| {
            render_block(&probe, window, cx)
        });
        let probe_image = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            let image = cache
                .ready_image(&key_for(cx, "graph release probe"))
                .expect("probe ready after parking");
            assert_eq!(cache.recorded_windows(&image).len(), 2);
            image
        });
        let release = cx.update(|cx| {
            // Snapshot the live windows before borrowing the cache: the
            // release decision below needs both, and they cannot alias.
            let live = cx.windows();
            let cache = cx.global_mut::<MermaidCache>();
            // Overflow both maps without spawning: dropped tasks make the
            // entry state machine (and nothing else) decide the outcome.
            for i in 0..CAP_ENTRIES {
                let key = CacheKey::new(
                    &format!("graph overflow entry {i}"),
                    MermaidTheme::light(),
                    TEST_SCALE,
                );
                let (_, task) = cache.request(&key);
                drop(task);
            }
            for block_id in 0..CAP_LAST_GOOD as u64 {
                let (image, logical) = raster_to_image(raster(2, 2, 1.0)).expect("dummy image");
                cache.store_last_good(
                    100_000 + block_id,
                    MermaidTheme::light(),
                    1.0f32.to_bits(),
                    image,
                    logical,
                );
            }
            cache
                .collect_releases(&live)
                .into_iter()
                .find(|release| Arc::ptr_eq(&release.image, &probe_image))
                .expect("the aged-out image must come back as a release")
        });
        let dropped = v1.update(|window, cx| release_on_windows(release, window, cx));
        assert_eq!(
            dropped, 2,
            "the release must drop the image from both live windows' atlases"
        );
        cx.update(|cx| {
            assert!(!cx.global::<MermaidCache>().has_live_record(&probe_image));
        });

        // Phase 2 — a diagram recorded by both windows; closing the second
        // prunes its handle on the next draw.
        let shown = block(1, "graph TD\nA-->B");
        draw_measured(v1, size(px(400.), px(2000.)), |window, cx| {
            render_block(&shown, window, cx)
        });
        cx.run_until_parked();
        draw_measured(v1, size(px(400.), px(2000.)), |window, cx| {
            render_block(&shown, window, cx)
        });
        draw_measured(v2, size(px(400.), px(2000.)), |window, cx| {
            render_block(&shown, window, cx)
        });
        let shown_image = cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            let image = cache
                .ready_image(&key_for(cx, "graph TD\nA-->B"))
                .expect("shown ready after parking");
            assert_eq!(cache.recorded_windows(&image).len(), 2);
            image
        });
        // Close the second window for real: `remove_window` marks it, the
        // update's effect flush drops it from the app's window list, and
        // parking settles the executor. (`simulate_close` only fires a
        // platform close request, which a bare test root never answers.)
        v2.update(|window, _| window.remove_window());
        cx.run_until_parked();
        draw_measured(v1, size(px(400.), px(2000.)), |window, cx| {
            render_block(&shown, window, cx)
        });
        cx.update(|cx| {
            let windows = cx.global::<MermaidCache>().recorded_windows(&shown_image);
            assert_eq!(windows.len(), 1, "the closed window's handle is pruned");
        });

        // Phase 3 — age the diagram out through the render path itself,
        // deterministically: each filler is drawn, parked (its render
        // lands), and drawn again (Ready, stores its last-good), so no
        // pending entry is ever evicted before it completes.
        for i in 0..CAP_ENTRIES + 2 {
            let filler = block(10_000 + i as u64, &format!("graph fill {i}"));
            draw_measured(v1, size(px(400.), px(2000.)), |window, cx| {
                render_block(&filler, window, cx)
            });
            cx.run_until_parked();
            draw_measured(v1, size(px(400.), px(2000.)), |window, cx| {
                render_block(&filler, window, cx)
            });
        }
        cx.update(|cx| {
            let cache = cx.global::<MermaidCache>();
            assert!(
                !cache.has_live_record(&shown_image),
                "with no entry and no last-good, the record is released"
            );
            assert_eq!(cache.ready_image(&key_for(cx, "graph TD\nA-->B")), None);
        });
    }

    /// Malformed rasters become failures, never panics.
    #[gpui::test]
    fn malformed_rasters_fail_gracefully(cx: &mut TestAppContext) {
        assert_eq!(raster_to_image(raster(0, 10, 1.0)), None);
        assert_eq!(raster_to_image(raster(10, 0, 1.0)), None);
        assert_eq!(
            raster_to_image(tty59_mermaid::Raster {
                width: 4,
                height: 4,
                scale: 1.0,
                bgra: vec![0; 8], // short buffer
            }),
            None
        );
        assert_eq!(raster_to_image(raster(4, 4, 0.0)), None);
        assert_eq!(raster_to_image(raster(4, 4, f32::NAN)), None);
        // A tiny positive scale divides finite pixels into infinity; the
        // logical size must be validated before the image is allocated.
        assert_eq!(raster_to_image(raster(2, 2, 1e-42)), None);
        // The good case: logical size comes from the raster's own scale.
        let (image, logical) = raster_to_image(raster(400, 200, 2.0)).expect("valid raster");
        assert_eq!(
            image.size(0),
            size(400.into(), 200.into()),
            "the image keeps the raster's device dimensions"
        );
        assert_close(logical.width, 200.0, "logical width");
        assert_close(logical.height, 100.0, "logical height");

        // And through the cache: a renderer returning a malformed raster
        // lands in Failed, not Ready.
        cx.update(|cx| {
            cx.set_global(MermaidCache::new(Arc::new(
                |_: &str, _: &MermaidTheme, _: f32| Ok(raster(8, 8, 0.0)),
            )));
        });
        cx.update(|cx| {
            let key = CacheKey::new("graph TD\nA-->B", MermaidTheme::light(), 2.0);
            let (_, task) = cx.global_mut::<MermaidCache>().request(&key);
            if let Some(task) = task {
                spawn_render(task, cx);
            }
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let key = CacheKey::new("graph TD\nA-->B", MermaidTheme::light(), 2.0);
            assert_eq!(cx.global::<MermaidCache>().state_of(&key), Some("failed"));
        });
    }

    /// Consuming the raster must hand its BGRA buffer straight to the gpui
    /// image. The previous `&Raster` adapter cloned the whole vector — up
    /// to `MAX_PIXELS` × 4 bytes per diagram — for a buffer nobody reads
    /// again. The pointer comparison is the clone detector: cloning
    /// allocates a new buffer while the original is still live, so the two
    /// addresses cannot be equal. The byte comparison is separate evidence
    /// that the move preserves every channel, alpha included (a pointer
    /// match alone would not catch swapped channels or clamped pixels).
    /// This test fails while the converter clones and passes once it moves.
    #[test]
    fn raster_conversion_moves_the_buffer_without_cloning() {
        let mut source = raster(2, 1, 1.0);
        source.bgra = vec![0x10, 0x20, 0x30, 0xff, 0x40, 0x50, 0x60, 0x80];
        let input_ptr = source.bgra.as_ptr();
        let input_len = source.bgra.len();

        let (image, _) = raster_to_image(source).expect("valid raster");
        let bytes = image.as_bytes(0).expect("the image has one frame");
        assert_eq!(bytes.len(), input_len);
        assert_eq!(
            bytes.as_ptr(),
            input_ptr,
            "the raster's BGRA buffer must be moved into the image, not cloned"
        );
        assert_eq!(
            bytes.to_vec(),
            vec![0x10, 0x20, 0x30, 0xff, 0x40, 0x50, 0x60, 0x80],
            "pixels (including alpha) survive the move byte for byte"
        );
        assert_eq!(bytes[3], 0xff, "first pixel's BGRA alpha");
        assert_eq!(bytes[7], 0x80, "second pixel's BGRA alpha");
    }

    /// Production-renderer smoke: no injected render function — the default
    /// cache runs the real [`tty59_mermaid::render`] pipeline (merman → SVG
    /// → resvg, including system-font rasterization) exactly like the editor
    /// preview. Valid flow, sequence, class, state and gantt sources must
    /// reach `Ready` with actually drawn pixels. Each source is drawn once
    /// to request its render (pending), then again after the background
    /// work lands: the second draw is the real ready-image path, which
    /// records the window that uploads the image. The pixel check also
    /// rejects a blank opaque background by requiring more than one RGB
    /// color, not just nonzero alpha.
    #[gpui::test]
    fn production_renderer_draws_flow_sequence_class_state_and_gantt(cx: &mut TestAppContext) {
        let cases = [
            (
                "flowchart",
                "flowchart TD\n    A[Start] --> B{Ready?}\n    B -->|yes| C[Done]\n    B -->|no| A",
            ),
            (
                "sequence",
                "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>B: Request\n    B-->>A: Response",
            ),
            (
                "class",
                "classDiagram\n    class Animal {\n        +String name\n        +eat()\n    }\n    class Dog {\n        +bark()\n    }\n    Animal <|-- Dog",
            ),
            (
                "state",
                "stateDiagram-v2\n    [*] --> Idle\n    Idle --> Running: start\n    Running --> Idle: stop\n    Running --> [*]",
            ),
            (
                "gantt",
                "gantt\n    title Release plan\n    dateFormat YYYY-MM-DD\n    axisFormat %m-%d\n    section Build\n    Compile :a1, 2026-01-01, 3d\n    Test :a2, after a1, 2d\n    section Ship\n    Package :b1, after a2, 1d",
            ),
        ];

        cx.update(|cx| {
            gpui_component::init(cx);
            // The production default: the real renderer, no test seam.
            cx.set_global(MermaidCache::default());
        });
        let vcx = cx.add_empty_window();

        for (name, source) in cases {
            let blk = block(1, source);
            // First draw is pending: visible prepaint performs the block's
            // single request per miss, and parking waits for the background
            // render to complete.
            draw_measured(vcx, size(px(600.), px(4000.)), |window, cx| {
                render_block(&blk, window, cx)
            });
            vcx.run_until_parked();

            // Second draw is the ready-image path the preview actually
            // paints; it also records the drawing window for atlas release.
            draw_measured(vcx, size(px(600.), px(4000.)), |window, cx| {
                render_block(&blk, window, cx)
            });

            vcx.update(|_, cx| {
                let key = key_for(cx, source);
                let cache = cx.global::<MermaidCache>();
                assert_eq!(
                    cache.state_of(&key),
                    Some("ready"),
                    "{name}: the production render did not complete"
                );
                let image = cache.ready_image(&key).expect("ready image");
                assert!(
                    cache.has_live_record(&image),
                    "{name}: the ready image must be displayed and recorded"
                );
                let size = image.size(0);
                assert!(
                    size.width.0 > 1 && size.height.0 > 1,
                    "{name}: degenerate image size {size:?}"
                );
                let bytes = image.as_bytes(0).expect("the image has one frame");
                assert_eq!(
                    bytes.len(),
                    size.width.0 as usize * size.height.0 as usize * 4,
                    "{name}: BGRA byte count"
                );
                let drawn = bytes.chunks_exact(4).filter(|pixel| pixel[3] != 0).count();
                assert!(drawn > 0, "{name}: the production renderer drew no pixels");
                // A blank opaque background has nonzero alpha everywhere but
                // a single RGB value; real diagrams use several ink colors.
                let mut colors = std::collections::HashSet::new();
                for pixel in bytes.chunks_exact(4) {
                    if pixel[3] != 0 {
                        colors.insert((pixel[0], pixel[1], pixel[2]));
                    }
                }
                assert!(
                    colors.len() > 1,
                    "{name}: drawn pixels must use more than one RGB color (got {})",
                    colors.len()
                );
            });
        }
    }

    /// The error line shows one trimmed line, capped; nothing usable comes
    /// back empty, which renders as the localized label alone.
    #[test]
    fn failure_messages_are_one_sanitized_line() {
        assert_eq!(
            failure_message("line one\nline two\nline three"),
            "line one"
        );
        assert_eq!(
            failure_message("  \n\n  padded message  \n"),
            "padded message"
        );
        assert_eq!(failure_message(""), "");
        assert_eq!(failure_message(" \n \n "), "");
        let long = "x".repeat(500);
        let capped = failure_message(&long);
        assert_eq!(capped.chars().count(), 201);
        assert!(capped.ends_with('…'));
    }
}
