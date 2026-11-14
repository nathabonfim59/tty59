//! The full-screen diagram viewer's viewport maths, pure and gpui-free.
//!
//! Every quantity is in logical pixels of the preview area the diagram is
//! shown in. `natural` is the diagram's CSS size — the raster's device size
//! over the scale it was rendered at — so the maths never depends on which
//! raster is currently up. Kept pure so the fit, zoom-about-point and clamp
//! behaviour can be unit-tested directly.

/// Fit padding: the margin kept around a fitted diagram, on each side.
pub(super) const MARGIN: f32 = 24.;
/// The viewer's zoom floor: a large diagram still shrinks to a thumbnail,
/// but no further than this, or it would vanish into a dot.
pub(super) const MIN_SCALE: f32 = 0.1;
/// The viewer's zoom ceiling. Past this the raster cap makes every step
/// softer without telling the eye anything new.
pub(super) const MAX_SCALE: f32 = 16.;
/// How much of the diagram must stay inside the area while panning, in px
/// per axis — enough to grab it again, never stranding it off screen.
pub(super) const KEEP_VISIBLE: f32 = 48.;

/// Where the diagram sits in the viewer's area: its top-left corner and its
/// scale relative to the diagram's natural size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Viewport {
    pub(super) scale: f32,
    pub(super) offset: (f32, f32),
}

/// A size, as the pure maths sees it: `(width, height)` in logical px.
pub(super) type Size = (f32, f32);

/// Fit `natural` into `area` with [`MARGIN`] around it: the largest scale
/// that fits both axes, clamped to the zoom range, centred. Small diagrams
/// may be upscaled — "fit" means fill the area pleasantly, not preserve the
/// intrinsic size the way the inline preview does.
pub(super) fn fit(natural: Size, area: Size) -> Viewport {
    let (nw, nh) = clamp_size(natural);
    let (aw, ah) = area;
    let by_width = (aw - 2. * MARGIN).max(0.) / nw;
    let by_height = (ah - 2. * MARGIN).max(0.) / nh;
    let scale = by_width.min(by_height).clamp(MIN_SCALE, MAX_SCALE);
    centre(scale, natural, area)
}

/// Scale `v` by `factor` about the point `p` (in area coordinates): the
/// diagram point under `p` stays under `p`. The scale is clamped first, so
/// a factor that would cross an end moves the offset to keep the anchor —
/// or leaves everything alone when the scale cannot change.
pub(super) fn zoom_about(v: Viewport, factor: f32, p: (f32, f32)) -> Viewport {
    let scale = (v.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
    if scale == v.scale {
        return v;
    }
    let ratio = scale / v.scale;
    let offset = (
        p.0 - (p.0 - v.offset.0) * ratio,
        p.1 - (p.1 - v.offset.1) * ratio,
    );
    Viewport { scale, offset }
}

/// Pan by `delta` (area px, positive = right/down), then keep the diagram
/// grabbable with [`clamp_pan`].
pub(super) fn pan(v: Viewport, delta: (f32, f32), natural: Size, area: Size) -> Viewport {
    clamp_pan(
        Viewport {
            offset: (v.offset.0 + delta.0, v.offset.1 + delta.1),
            ..v
        },
        natural,
        area,
    )
}

/// Keep at least [`KEEP_VISIBLE`] px of the diagram inside the area on each
/// axis, however big or small it is — a diagram that fits the area can be
/// dragged around too, just not off it. A diagram smaller than the band
/// itself keeps all of it on screen.
pub(super) fn clamp_pan(v: Viewport, natural: Size, area: Size) -> Viewport {
    let (nw, nh) = clamp_size(natural);
    let (aw, ah) = area;
    let (w, h) = (nw * v.scale, nh * v.scale);
    let offset = (clamp_axis(v.offset.0, w, aw), clamp_axis(v.offset.1, h, ah));
    Viewport { offset, ..v }
}

/// One axis of [`clamp_pan`]: `at` is the diagram's top/left edge. The
/// diagram must overlap the area by at least `keep` on each side, so `at`
/// lives in `[keep − size, area − keep]`.
fn clamp_axis(at: f32, size: f32, area: f32) -> f32 {
    let keep = KEEP_VISIBLE.min(area / 2.).min(size);
    at.clamp(keep - size, area - keep)
}

/// How much one wheel event zooms: `1.0025ᵈʸ`, `dy` in px (a line counts as
/// 16), in gpui's sign — positive is the wheel rolled up, away from the
/// reader, which zooms in, as it does in the terminal and the editor. (The
/// browser's `deltaY` runs the other way; this is not that.)
pub(super) fn wheel_factor(dy_px: f32) -> f32 {
    1.0025f32.powf(dy_px)
}

/// The device scale to render at for a viewer zoom of `scale`: the window's
/// scale times the smallest power of √2 not below `scale`. Zooming re-
/// renders at ×1.41 boundaries rather than continuously, which is sharp
/// enough at every step and re-rasterises a handful of times per sweep.
pub(super) fn raster_bucket(scale: f32, window_scale: f32) -> f32 {
    if scale <= 1. {
        return window_scale;
    }
    // The ladder is 1, √2, 2, 2√2, 4… — half-powers of two, counted rather
    // than multiplied, because √2² is not exactly 2 in f32 and the steps
    // must land on the powers they name. The comparison carries a small
    // relative tolerance so a scale sitting exactly on a power does not
    // step past it.
    let mut halves = 0u32;
    let step = |halves: u32| {
        let whole = f32::powi(2.0, (halves / 2) as i32);
        if halves.is_multiple_of(2) {
            whole
        } else {
            whole * SQRT_2
        }
    };
    while step(halves) < scale - scale * 1e-3 {
        halves += 1;
    }
    window_scale * step(halves)
}

const SQRT_2: f32 = std::f32::consts::SQRT_2;

/// Degenerate sizes are clamped so divisions stay finite even if a renderer
/// returns something extreme.
fn clamp_size((w, h): Size) -> Size {
    (w.max(1.), h.max(1.))
}

/// `fit`'s centre step, shared with nothing else (fit is the only entry
/// that starts centred).
fn centre(scale: f32, natural: Size, area: Size) -> Viewport {
    let (nw, nh) = clamp_size(natural);
    let (w, h) = (nw * scale, nh * scale);
    Viewport {
        scale,
        offset: ((area.0 - w) / 2., (area.1 - h) / 2.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Size = (1000., 600.);

    #[test]
    fn fit_centres_and_respects_the_margin() {
        // A wide diagram fits by width, leaving margin on both sides.
        let v = fit((2000., 1000.), AREA);
        // (1000 − 48) / 2000 = 0.476
        assert!((v.scale - 0.476).abs() < 0.001, "scale {}", v.scale);
        let w = 2000. * v.scale;
        assert!((v.offset.0 - (1000. - w) / 2.).abs() < 0.01, "centred in x");
        // Height at that scale: 476, so vertical margin is bigger.
        assert!(v.offset.1 > MARGIN, "margin kept: {}", v.offset.1);

        // A tall diagram fits by height instead.
        let v = fit((1000., 4000.), AREA);
        // (600 − 48) / 4000 = 0.138
        assert!((v.scale - 0.138).abs() < 0.001, "scale {}", v.scale);

        // A small diagram is upscaled to fill — fit means fill, pleasantly.
        let v = fit((100., 50.), AREA);
        // (1000 − 48) / 100 = 9.52
        assert!((v.scale - 9.52).abs() < 0.01, "upscaled: {}", v.scale);
        assert!((v.offset.0 - MARGIN).abs() < 0.01, "x margin");
        assert!(v.offset.1 > MARGIN, "centred in y beyond the margin");
    }

    #[test]
    fn zoom_about_keeps_the_point_under_the_pointer_fixed() {
        let v = Viewport {
            scale: 1.,
            offset: (100., 50.),
        };
        let p = (430., 210.);
        let z = zoom_about(v, 2., p);
        // The diagram point that sat under p still sits under p, to float
        // round-trip precision.
        let round = zoom_about(z, 0.5, p);
        assert!((round.offset.0 - v.offset.0).abs() < 1e-3);
        assert!((round.offset.1 - v.offset.1).abs() < 1e-3);
        assert!((round.scale - v.scale).abs() < 1e-6);
        // And at the doubled scale, the anchor is exact by construction.
        let dx = p.0 - z.offset.0;
        assert!((2. * (p.0 - v.offset.0) - dx).abs() < 1e-3);
    }

    #[test]
    fn zoom_clamps_at_both_ends() {
        let v = Viewport {
            scale: MAX_SCALE,
            offset: (0., 0.),
        };
        let z = zoom_about(v, 8., (100., 100.));
        assert_eq!(z.scale, MAX_SCALE, "no growth past the ceiling");
        assert_eq!(z, v, "and nothing moves when the scale cannot");
        let v = Viewport {
            scale: MIN_SCALE,
            offset: (0., 0.),
        };
        assert_eq!(zoom_about(v, 0.1, (0., 0.)).scale, MIN_SCALE);
    }

    #[test]
    fn panning_keeps_the_diagram_grabbable() {
        let natural = (2000., 1000.);
        // A fitted diagram fits by construction, so pan somewhere it
        // doesn't: zoom in about the centre first.
        let v = zoom_about(fit(natural, AREA), 4., (AREA.0 / 2., AREA.1 / 2.));
        let w = 2000. * v.scale;
        assert!(w > AREA.0, "the zoomed diagram overflows the area");
        // Pan far past the right edge: 48 px of it stay visible on the left.
        let p = pan(v, (100_000., 0.), natural, AREA);
        assert!(
            (p.offset.0 - (AREA.0 - KEEP_VISIBLE)).abs() < 1e-3,
            "right keep: {}",
            p.offset.0
        );
        // And past the left edge: 48 px stay visible on the right.
        let p = pan(v, (-100_000., 0.), natural, AREA);
        assert!(
            (p.offset.0 - (KEEP_VISIBLE - w)).abs() < 1e-3,
            "left keep: {}",
            p.offset.0
        );

        // A diagram that fits the area pans too, bounded the same way: the
        // spec keeps 48 px of it on screen, it does not lock it to the
        // centre.
        let tall = fit((400., 4000.), AREA);
        let w = 400. * tall.scale;
        assert!(
            KEEP_VISIBLE < w && w < AREA.0,
            "the fitted diagram is narrower than the area, wider than the band: {w}"
        );
        let p = pan(tall, (30., 0.), (400., 4000.), AREA);
        assert!(
            (p.offset.0 - (tall.offset.0 + 30.)).abs() < 1e-3,
            "a fitted diagram moves: {} -> {}",
            tall.offset.0,
            p.offset.0
        );
        let p = pan(tall, (100_000., 0.), (400., 4000.), AREA);
        assert!(
            (p.offset.0 - (AREA.0 - KEEP_VISIBLE)).abs() < 1e-3,
            "and stops with 48 px in view: {}",
            p.offset.0
        );
        // A diagram thinner than the band keeps all of itself in view.
        let sliver = Viewport {
            scale: 1.,
            offset: (0., 0.),
        };
        let p = pan(sliver, (100_000., 0.), (20., 20.), AREA);
        assert!((p.offset.0 - (AREA.0 - 20.)).abs() < 1e-3, "{}", p.offset.0);
    }

    #[test]
    fn wheel_factor_follows_the_pointer_direction() {
        assert!(wheel_factor(100.) > 1., "wheel up (dy > 0) zooms in");
        assert!(wheel_factor(-100.) < 1., "wheel down (dy < 0) zooms out");
        assert!((wheel_factor(0.) - 1.).abs() < 1e-6);
        // One notch, three lines: a ~4.7 % step — small enough to feel
        // smooth over a flick, big enough to move.
        assert!(
            (wheel_factor(48.) - 1.1277).abs() < 0.01,
            "{}",
            wheel_factor(48.)
        );
    }

    #[test]
    fn raster_bucket_steps_in_sqrt2_powers() {
        let ws = 2.;
        assert_eq!(
            raster_bucket(0.5, ws),
            ws,
            "below 1 renders at the window scale"
        );
        assert_eq!(raster_bucket(1.0, ws), ws);
        assert!((raster_bucket(1.2, ws) - ws * SQRT_2).abs() < 1e-3);
        assert!((raster_bucket(SQRT_2, ws) - ws * SQRT_2).abs() < 1e-3);
        // Just past √2 steps to the next power, 2.
        assert!((raster_bucket(SQRT_2 + 0.01, ws) - ws * 2.).abs() < 1e-2);
        assert!((raster_bucket(4., ws) - ws * 4.).abs() < 1e-2);
        // 8 < 10 ≤ 8·√2 ≈ 11.31: the smallest power that covers.
        assert!((raster_bucket(10., ws) - ws * 8. * SQRT_2).abs() < 1e-2);
    }
}
